//! Schedules: recordings and playout started at set times, and stopped at
//! the end. Only a controller runs them, through the same node API the UI
//! uses ([`forward::call_node`]); nodes know nothing of schedules.
//!
//! Times are the controller's local time (a venue keeps its machines in one
//! time zone). Each occurrence that comes due gets one `schedule_runs` row,
//! inserted before anything starts, so it can't run twice; a run remembers
//! the recordings it started, so it still stops them after a restart.
//!
//! An occurrence whose window is still open when the controller notices it
//! (after a restart, or just created) starts late. One whose window passed
//! while the controller was off is recorded as skipped. Occurrences whose
//! window closed before the schedule was last saved are ignored.

use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, LocalResult, NaiveDate, NaiveTime, TimeZone, Utc};
use futures_util::future::join_all;
use serde_json::json;
use tokio::sync::{Mutex, Notify};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use super::forward::call_node;
use crate::api::types::{
    ChromaSubsampling, ClockTimeDto, Container, PresetDto, PresetOutputInput, RecordingSessionDto,
    ScheduleAction, ScheduleDto, ScheduleKind, ScheduleRunDto, ScheduleRunStatus,
    ScheduleRunTarget, ScheduleTargetKind, StartRecordingRequest, VideoCodec, WsEvent,
};
use crate::db;
use crate::state::AppState;

/// How long before a start the scheduler wakes, so recordings can be set to
/// begin on the scheduled frame.
const LEAD: Duration = Duration::from_secs(2);
/// A synchronized start's lead: at least enough for every node to build its
/// legs, at most within a node's limit (10 s).
const MIN_LEAD_US: i64 = 1_500_000;
const MAX_LEAD_US: i64 = 8_000_000;
/// How late a start without an end may still happen.
const NO_END_GRACE: chrono::Duration = chrono::Duration::minutes(2);
/// The longest sleep, so a changed wall clock or a sleeping Mac is noticed.
const IDLE: Duration = Duration::from_secs(60);

/// The controller's handle on its scheduler.
#[derive(Default)]
pub struct Scheduler {
    wake: Notify,
    /// Held while runs start or stop, so a "stop now" can't race the loop.
    lock: Mutex<()>,
}

impl Scheduler {
    /// Look again now: a schedule changed.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// End a schedule's running occurrence now. `false` if none is running.
    pub async fn stop_now(&self, state: &AppState, schedule_id: &str) -> anyhow::Result<bool> {
        let _guard = self.lock.lock().await;
        let running = db::schedule_runs_running(&state.db).await?;
        let Some(run) = running.into_iter().find(|r| r.schedule_id == schedule_id) else {
            return Ok(false);
        };
        stop_run(state, run).await?;
        self.wake();
        Ok(true)
    }
}

pub fn spawn(state: Arc<AppState>, scheduler: Arc<Scheduler>, cancel: CancellationToken) {
    tokio::spawn(async move {
        loop {
            let next = match tick(&state, &scheduler).await {
                Ok(next) => next,
                Err(e) => {
                    warn!(error = %e, "scheduler");
                    Utc::now() + IDLE
                }
            };
            let sleep = (next - Utc::now()).to_std().unwrap_or_default().min(IDLE);
            tokio::select! {
                _ = cancel.cancelled() => return,
                _ = scheduler.wake.notified() => {}
                _ = tokio::time::sleep(sleep) => {}
            }
        }
    });
}

/// Start and stop what's due; returns when to look again.
async fn tick(state: &AppState, scheduler: &Scheduler) -> anyhow::Result<DateTime<Utc>> {
    let _guard = scheduler.lock.lock().await;
    let now = Utc::now();
    let lead = chrono::Duration::from_std(LEAD)?;
    let mut next = now + IDLE;

    for run in db::schedule_runs_running(&state.db).await? {
        match run.planned_end.as_deref().and_then(parse_time) {
            Some(end) if end <= now => stop_run(state, run).await?,
            Some(end) => next = next.min(end),
            // Nothing to stop: shouldn't be running.
            None => stop_run(state, run).await?,
        }
    }

    for s in db::schedules_list(&state.db).await? {
        if !s.enabled {
            continue;
        }
        let Ok(rule) = Rule::of(&s) else { continue };
        let saved = parse_time(&s.updated_at).unwrap_or(now);
        if let Some(start) = rule.latest_at_or_before(&chrono::Local, now + lead) {
            let window_end = start + rule.duration.unwrap_or(NO_END_GRACE);
            let planned = start.to_rfc3339();
            if window_end > saved && !db::schedule_run_exists(&state.db, &s.id, &planned).await? {
                if now < window_end {
                    start_run(state, &s, &rule, start).await?;
                } else {
                    skip_run(state, &s, &rule, start).await?;
                }
            }
        }
        if let Some(start) = rule.next_after(&chrono::Local, now + lead) {
            next = next.min(start - lead);
        }
    }
    Ok(next)
}

fn parse_time(s: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(s)
        .ok()
        .map(|t| t.with_timezone(&Utc))
}

fn new_run(
    s: &ScheduleDto,
    rule: &Rule,
    start: DateTime<Utc>,
    status: ScheduleRunStatus,
) -> ScheduleRunDto {
    ScheduleRunDto {
        id: uuid::Uuid::new_v4().to_string(),
        schedule_id: s.id.clone(),
        planned_start: start.to_rfc3339(),
        planned_end: rule.duration.map(|d| (start + d).to_rfc3339()),
        started_at: None,
        stopped_at: None,
        status,
        targets: vec![],
    }
}

async fn skip_run(
    state: &AppState,
    s: &ScheduleDto,
    rule: &Rule,
    start: DateTime<Utc>,
) -> anyhow::Result<()> {
    let run = new_run(s, rule, start, ScheduleRunStatus::Skipped);
    if db::schedule_run_insert(&state.db, &run).await? {
        warn!(schedule = %s.name, planned = %run.planned_start, "schedule skipped: its time passed while this controller was off");
        emit(state, s, run);
    }
    Ok(())
}

async fn start_run(
    state: &AppState,
    s: &ScheduleDto,
    rule: &Rule,
    start: DateTime<Utc>,
) -> anyhow::Result<()> {
    let mut run = new_run(s, rule, start, ScheduleRunStatus::Running);
    if !db::schedule_run_insert(&state.db, &run).await? {
        return Ok(());
    }
    let now = Utc::now();
    if now > start + chrono::Duration::seconds(5) {
        warn!(schedule = %s.name, late_s = (now - start).num_seconds(), "schedule starting late");
    }

    // Recordings first: they're set to begin on the scheduled frame of the
    // shared clock, so they need the lead.
    let mut presets = db::presets_list(&state.db).await?;
    presets.push(default_preset());
    let clock = state.clock.now();
    let lead_us = (start - now)
        .num_microseconds()
        .unwrap_or(0)
        .clamp(MIN_LEAD_US, MAX_LEAD_US);
    let start_at = ClockTimeDto {
        domain: clock.domain,
        time_us: clock.time_us + lead_us as u64,
    };
    let mut recordings = vec![];
    for action in &s.actions {
        if let ScheduleAction::Record { targets, preset_id } = action {
            let preset = presets.iter().find(|p| p.id == *preset_id);
            for t in targets {
                recordings.push(start_recording(
                    state,
                    &t.node_id,
                    &t.source_id,
                    preset,
                    &start_at,
                ));
            }
        }
    }
    let mut targets = join_all(recordings).await;

    // Playout has no synchronized start: start it on time.
    if let Ok(wait) = (start - Utc::now()).to_std() {
        tokio::time::sleep(wait).await;
    }
    let channels = s.actions.iter().filter_map(|a| match a {
        ScheduleAction::Playout {
            node_id,
            channel_id,
            item_id,
            stop_at_end,
        } => Some(start_channel(
            state,
            node_id,
            channel_id,
            item_id.as_deref(),
            *stop_at_end,
        )),
        ScheduleAction::Record { .. } => None,
    });
    targets.extend(join_all(channels).await);

    run.started_at = Some(Utc::now().to_rfc3339());
    let failed = targets.iter().filter(|t| t.error.is_some()).count();
    run.status = if failed == targets.len() {
        ScheduleRunStatus::Failed
    } else if run.planned_end.is_some() {
        ScheduleRunStatus::Running
    } else if failed > 0 {
        ScheduleRunStatus::Partial
    } else {
        ScheduleRunStatus::Done
    };
    for t in targets.iter().filter(|t| t.error.is_some()) {
        warn!(schedule = %s.name, node_id = %t.node_id, id = %t.id, error = t.error.as_deref().unwrap_or(""), "schedule couldn't start one of its targets");
    }
    info!(schedule = %s.name, started = targets.len() - failed, failed, "schedule started");
    run.targets = targets;
    db::schedule_run_update(&state.db, &run).await?;
    emit(state, s, run);
    Ok(())
}

async fn start_recording(
    state: &AppState,
    node_id: &str,
    source_id: &str,
    preset: Option<&PresetDto>,
    start_at: &ClockTimeDto,
) -> ScheduleRunTarget {
    let mut target = ScheduleRunTarget {
        kind: ScheduleTargetKind::Recording,
        node_id: node_id.to_string(),
        id: source_id.to_string(),
        session_id: None,
        error: None,
        stop_at_end: true,
    };
    let Some(preset) = preset else {
        target.error = Some("its preset was deleted".into());
        return target;
    };
    let req = StartRecordingRequest {
        source_id: source_id.to_string(),
        preset_id: Some(preset.id.clone()),
        preset_name: Some(preset.name.clone()),
        outputs: preset.outputs.clone(),
        start_at: Some(start_at.clone()),
    };
    let body = serde_json::to_value(&req).expect("a request serializes");
    let result = call_node(
        state,
        node_id,
        axum::http::Method::POST,
        "/recordings",
        Some(&body),
    )
    .await;
    match result
        .and_then(|b| serde_json::from_slice::<RecordingSessionDto>(&b).map_err(|e| e.to_string()))
    {
        Ok(session) => target.session_id = Some(session.id),
        Err(e) => target.error = Some(e),
    }
    target
}

async fn start_channel(
    state: &AppState,
    node_id: &str,
    channel_id: &str,
    item_id: Option<&str>,
    stop_at_end: bool,
) -> ScheduleRunTarget {
    let (path, body) = match item_id {
        Some(item_id) => (
            format!("/channels/{channel_id}/play"),
            json!({ "item_id": item_id }),
        ),
        None => (
            format!("/channels/{channel_id}/transport"),
            json!({ "action": "play" }),
        ),
    };
    let result = call_node(state, node_id, axum::http::Method::POST, &path, Some(&body)).await;
    ScheduleRunTarget {
        kind: ScheduleTargetKind::Channel,
        node_id: node_id.to_string(),
        id: channel_id.to_string(),
        session_id: None,
        error: result.err(),
        stop_at_end,
    }
}

/// Stop what `run` started and still stops at its end. A recording or
/// channel someone already stopped is fine.
async fn stop_run(state: &AppState, mut run: ScheduleRunDto) -> anyhow::Result<()> {
    let stops = run
        .targets
        .iter()
        .filter(|t| t.error.is_none() && t.stop_at_end)
        .map(|t| async move {
            let (path, body) = match (t.kind, &t.session_id) {
                (ScheduleTargetKind::Recording, Some(session)) => (format!("/recordings/{session}/stop"), None),
                (ScheduleTargetKind::Recording, None) => return,
                (ScheduleTargetKind::Channel, _) => {
                    (format!("/channels/{}/transport", t.id), Some(json!({ "action": "stop" })))
                }
            };
            if let Err(e) = call_node(state, &t.node_id, axum::http::Method::POST, &path, body.as_ref()).await {
                info!(node_id = %t.node_id, id = %t.id, error = %e, "schedule end: not stopped (perhaps already stopped)");
            }
        });
    join_all(stops).await;

    run.stopped_at = Some(Utc::now().to_rfc3339());
    run.status = if run.targets.iter().any(|t| t.error.is_some()) {
        ScheduleRunStatus::Partial
    } else {
        ScheduleRunStatus::Done
    };
    db::schedule_run_update(&state.db, &run).await?;
    let schedule = db::schedule_get(&state.db, &run.schedule_id).await?;
    if let Some(s) = schedule {
        info!(schedule = %s.name, "schedule ended");
        emit(state, &s, run);
    }
    Ok(())
}

/// The UI's built-in preset (`DEFAULT_PRESET` and `blankLeg` there): one
/// H.264 leg, never stored.
pub const DEFAULT_PRESET_ID: &str = "default";

fn default_preset() -> PresetDto {
    PresetDto {
        id: DEFAULT_PRESET_ID.into(),
        name: "H.264 (default)".into(),
        outputs: vec![PresetOutputInput {
            name: "Output".into(),
            codec: VideoCodec::H264,
            container: Container::Mov,
            resolution: None,
            framerate: None,
            bitrate_kbps: None,
            chroma: ChromaSubsampling::Yuv420,
            path_template: "~/capture-room/{date}/{source}_{datetime}.{ext}".into(),
            preview: false,
            advanced: Default::default(),
        }],
        created_at: String::new(),
        updated_at: String::new(),
        version: 1,
    }
}

fn emit(state: &AppState, s: &ScheduleDto, run: ScheduleRunDto) {
    state.emit_controller(&WsEvent::ScheduleUpdated {
        schedule_id: s.id.clone(),
        name: s.name.clone(),
        run: Some(Box::new(run)),
    });
}

/// When a schedule runs, parsed from its stored form.
#[derive(Debug, Clone)]
pub struct Rule {
    kind: ScheduleKind,
    /// A one-off's date, or the first date a monthly schedule may run.
    date: Option<NaiveDate>,
    weekdays: u8,
    month_day: Option<u8>,
    month_week: Option<i8>,
    start: NaiveTime,
    pub duration: Option<chrono::Duration>,
}

impl Rule {
    pub fn of(s: &ScheduleDto) -> Result<Self, String> {
        Self::parse(
            s.kind,
            s.date.as_deref(),
            s.weekdays,
            s.month_day,
            s.month_week,
            &s.start_time,
            s.duration_s,
        )
    }

    pub fn parse(
        kind: ScheduleKind,
        date: Option<&str>,
        weekdays: u8,
        month_day: Option<u8>,
        month_week: Option<i8>,
        start_time: &str,
        duration_s: Option<u32>,
    ) -> Result<Self, String> {
        let start = NaiveTime::parse_from_str(start_time, "%H:%M")
            .map_err(|_| format!("{start_time:?} isn't a time (HH:MM)"))?;
        let parse_date = |d: &str| {
            NaiveDate::parse_from_str(d, "%Y-%m-%d")
                .map_err(|_| format!("{d:?} isn't a date (YYYY-MM-DD)"))
        };
        let weekdays = weekdays & 0x7f;
        let (date, month_day, month_week) = match kind {
            ScheduleKind::Once => {
                let d = date.ok_or("a one-off schedule needs a date")?;
                (Some(parse_date(d)?), None, None)
            }
            ScheduleKind::Weekly => {
                if weekdays == 0 {
                    return Err("pick at least one day".into());
                }
                (None, None, None)
            }
            ScheduleKind::Monthly => {
                match (month_day, month_week) {
                    (Some(_), Some(_)) => {
                        return Err("a monthly schedule runs on a day of the month or on a \
                                    weekday, not both"
                            .into());
                    }
                    (None, None) => {
                        return Err("pick a day of the month, or a weekday in it".into());
                    }
                    (Some(d), None) if !(1..=31).contains(&d) => {
                        return Err(format!("{d} isn't a day of the month (1–31)"));
                    }
                    (None, Some(w)) if !matches!(w, 1..=4 | -1) => {
                        return Err(format!(
                            "{w} isn't a week of the month (1–4, or -1 for the last)"
                        ));
                    }
                    (None, Some(_)) if weekdays.count_ones() != 1 => {
                        return Err("pick one day of the week".into());
                    }
                    _ => {}
                }
                (date.map(parse_date).transpose()?, month_day, month_week)
            }
        };
        if duration_s == Some(0) {
            return Err("a schedule that ends must last at least a second".into());
        }
        Ok(Self {
            kind,
            date,
            weekdays,
            month_day,
            month_week,
            start,
            duration: duration_s.map(|s| chrono::Duration::seconds(s.into())),
        })
    }

    fn matches(&self, d: NaiveDate) -> bool {
        use chrono::Datelike;
        match self.kind {
            ScheduleKind::Once => self.date == Some(d),
            ScheduleKind::Weekly => self.weekdays & (1 << d.weekday().num_days_from_monday()) != 0,
            ScheduleKind::Monthly => {
                self.in_month(d.year(), d.month()) == Some(d) && self.date.is_none_or(|a| d >= a)
            }
        }
    }

    /// A monthly schedule's date in a month, if that month has one.
    fn in_month(&self, year: i32, month: u32) -> Option<NaiveDate> {
        use chrono::Datelike;
        if let Some(day) = self.month_day {
            return NaiveDate::from_ymd_opt(year, month, day.into());
        }
        let week = self.month_week?;
        let weekday = self.weekdays.trailing_zeros();
        let first = NaiveDate::from_ymd_opt(year, month, 1)?;
        if week > 0 {
            let ahead = (weekday + 7 - first.weekday().num_days_from_monday()) % 7;
            NaiveDate::from_ymd_opt(year, month, 1 + ahead + 7 * (week as u32 - 1))
        } else {
            let last = first
                .checked_add_months(chrono::Months::new(1))?
                .pred_opt()?;
            let back = (last.weekday().num_days_from_monday() + 7 - weekday) % 7;
            Some(last - chrono::Duration::days(back.into()))
        }
    }

    /// The start on local date `d`. A time the clocks skip over moves to the
    /// first minute after; a time they repeat is the first of the two.
    fn start_on<Tz: TimeZone>(&self, tz: &Tz, d: NaiveDate) -> Option<DateTime<Utc>> {
        let naive = d.and_time(self.start);
        let local = match tz.from_local_datetime(&naive) {
            LocalResult::Single(t) | LocalResult::Ambiguous(t, _) => Some(t),
            LocalResult::None => (1..=180).find_map(|m| {
                tz.from_local_datetime(&(naive + chrono::Duration::minutes(m)))
                    .earliest()
            }),
        };
        local.map(|t| t.with_timezone(&Utc))
    }

    /// The candidate starts on local dates around `t`, far enough `ahead` of
    /// it (or behind) to hold the next (or previous) one, plus a day the
    /// other way for the time zone. A one-off has only its date.
    fn starts<Tz: TimeZone>(&self, tz: &Tz, t: DateTime<Utc>, ahead: bool) -> Vec<DateTime<Utc>> {
        use chrono::Datelike;
        let today = t.with_timezone(tz).date_naive();
        let dates: Vec<NaiveDate> = match self.kind {
            ScheduleKind::Once => self.date.into_iter().collect(),
            // Every week has one.
            ScheduleKind::Weekly => {
                let range: Vec<i64> = if ahead {
                    (-1..=8).collect()
                } else {
                    (-8..=1).rev().collect()
                };
                range
                    .into_iter()
                    .map(|i| today + chrono::Duration::days(i))
                    .filter(|d| self.matches(*d))
                    .collect()
            }
            // Each month's date, worked out directly. A day of the month
            // skips at most two months running (the 31st), so a year is
            // plenty. Looking ahead starts from the first date it may run.
            ScheduleKind::Monthly => {
                let from = match self.date {
                    Some(first) if ahead => today.max(first),
                    _ => today,
                };
                let base = from.year() * 12 + from.month0() as i32;
                let range: Vec<i32> = if ahead {
                    (-1..=13).collect()
                } else {
                    (-13..=1).rev().collect()
                };
                range
                    .into_iter()
                    .filter_map(|i| {
                        let m = base + i;
                        self.in_month(m.div_euclid(12), m.rem_euclid(12) as u32 + 1)
                    })
                    .filter(|d| self.matches(*d))
                    .collect()
            }
        };
        dates
            .into_iter()
            .filter_map(|d| self.start_on(tz, d))
            .collect()
    }

    /// The latest start at or before `t`.
    pub fn latest_at_or_before<Tz: TimeZone>(
        &self,
        tz: &Tz,
        t: DateTime<Utc>,
    ) -> Option<DateTime<Utc>> {
        self.starts(tz, t, false)
            .into_iter()
            .filter(|s| *s <= t)
            .max()
    }

    /// The first start after `t`.
    pub fn next_after<Tz: TimeZone>(&self, tz: &Tz, t: DateTime<Utc>) -> Option<DateTime<Utc>> {
        self.starts(tz, t, true)
            .into_iter()
            .filter(|s| *s > t)
            .min()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono_tz::America::Los_Angeles as LA;

    fn utc(s: &str) -> DateTime<Utc> {
        parse_time(s).unwrap()
    }

    fn weekly(days: u8, time: &str) -> Rule {
        Rule::parse(
            ScheduleKind::Weekly,
            None,
            days,
            None,
            None,
            time,
            Some(3600),
        )
        .unwrap()
    }

    const SUN: u8 = 1 << 6;
    const WED: u8 = 1 << 2;

    fn monthly_on(day: u8, time: &str) -> Rule {
        Rule::parse(ScheduleKind::Monthly, None, 0, Some(day), None, time, None).unwrap()
    }

    fn monthly_nth(week: i8, weekday: u8, time: &str) -> Rule {
        Rule::parse(
            ScheduleKind::Monthly,
            None,
            weekday,
            None,
            Some(week),
            time,
            None,
        )
        .unwrap()
    }

    #[test]
    fn weekly_next_crosses_the_week() {
        // Sunday 18 Oct 2026, 10:00 PDT (17:00Z), after the 8:45 start.
        let r = weekly(SUN | WED, "08:45");
        let t = utc("2026-10-18T17:00:00Z");
        assert_eq!(
            r.latest_at_or_before(&LA, t),
            Some(utc("2026-10-18T15:45:00Z"))
        );
        assert_eq!(r.next_after(&LA, t), Some(utc("2026-10-21T15:45:00Z")));
        let sun_only = weekly(SUN, "08:45");
        assert_eq!(
            sun_only.next_after(&LA, t),
            Some(utc("2026-10-25T15:45:00Z"))
        );
        assert_eq!(
            sun_only.latest_at_or_before(&LA, utc("2026-10-24T12:00:00Z")),
            Some(utc("2026-10-18T15:45:00Z"))
        );
    }

    #[test]
    fn once_has_one_start() {
        let r = Rule::parse(
            ScheduleKind::Once,
            Some("2026-10-24"),
            0,
            None,
            None,
            "19:00",
            None,
        )
        .unwrap();
        let start = utc("2026-10-25T02:00:00Z");
        assert_eq!(r.next_after(&LA, utc("2026-01-01T00:00:00Z")), Some(start));
        assert_eq!(r.next_after(&LA, start), None);
        assert_eq!(r.latest_at_or_before(&LA, start), Some(start));
        assert_eq!(
            r.latest_at_or_before(&LA, start - chrono::Duration::seconds(1)),
            None
        );
    }

    #[test]
    fn a_skipped_time_moves_to_the_first_minute_after() {
        // 8 Mar 2026: 2:00 PST jumps to 3:00 PDT.
        let r = weekly(SUN, "02:30");
        assert_eq!(
            r.next_after(&LA, utc("2026-03-07T12:00:00Z")),
            Some(utc("2026-03-08T10:00:00Z"))
        );
    }

    #[test]
    fn a_repeated_time_is_the_first() {
        // 1 Nov 2026: 1:30 happens in PDT (08:30Z), then again in PST.
        let r = weekly(SUN, "01:30");
        assert_eq!(
            r.next_after(&LA, utc("2026-10-31T12:00:00Z")),
            Some(utc("2026-11-01T08:30:00Z"))
        );
    }

    #[test]
    fn rejects_bad_rules() {
        assert!(Rule::parse(ScheduleKind::Weekly, None, 0, None, None, "08:00", None).is_err());
        assert!(Rule::parse(ScheduleKind::Weekly, None, SUN, None, None, "8am", None).is_err());
        assert!(Rule::parse(ScheduleKind::Once, None, 0, None, None, "08:00", None).is_err());
        assert!(Rule::parse(
            ScheduleKind::Once,
            Some("2026-13-01"),
            0,
            None,
            None,
            "08:00",
            None
        )
        .is_err());
        assert!(Rule::parse(
            ScheduleKind::Weekly,
            None,
            SUN,
            None,
            None,
            "08:00",
            Some(0)
        )
        .is_err());
    }

    #[test]
    fn the_31st_skips_short_months() {
        let r = monthly_on(31, "10:00");
        // After Sat 31 Jan 2026 10:00 PST: February has no 31st.
        let t = utc("2026-01-31T19:00:00Z");
        assert_eq!(r.next_after(&LA, t), Some(utc("2026-03-31T17:00:00Z")));
        assert_eq!(
            r.latest_at_or_before(&LA, utc("2026-03-30T00:00:00Z")),
            Some(utc("2026-01-31T18:00:00Z"))
        );
        // April has none either; May does.
        assert_eq!(
            r.next_after(&LA, utc("2026-04-01T00:00:00Z")),
            Some(utc("2026-05-31T17:00:00Z"))
        );
    }

    #[test]
    fn the_29th_and_30th_of_february() {
        let r = monthly_on(29, "10:00");
        // 2027 isn't a leap year; 2028 is.
        assert_eq!(
            r.next_after(&LA, utc("2027-01-30T00:00:00Z")),
            Some(utc("2027-03-29T17:00:00Z"))
        );
        assert_eq!(
            r.next_after(&LA, utc("2028-01-30T00:00:00Z")),
            Some(utc("2028-02-29T18:00:00Z"))
        );
        // There's never a 30 February.
        let r = monthly_on(30, "10:00");
        assert_eq!(
            r.next_after(&LA, utc("2028-01-31T00:00:00Z")),
            Some(utc("2028-03-30T17:00:00Z"))
        );
        assert_eq!(
            r.latest_at_or_before(&LA, utc("2028-03-01T00:00:00Z")),
            Some(utc("2028-01-30T18:00:00Z"))
        );
    }

    #[test]
    fn last_weekday_in_four_and_five_week_months() {
        let r = monthly_nth(-1, SUN, "10:00");
        // February 2026 has four Sundays, November 2026 five.
        assert_eq!(
            r.next_after(&LA, utc("2026-02-01T00:00:00Z")),
            Some(utc("2026-02-22T18:00:00Z"))
        );
        assert_eq!(
            r.next_after(&LA, utc("2026-11-01T00:00:00Z")),
            Some(utc("2026-11-29T18:00:00Z"))
        );
        // After the last in October (the 25th), it's November's.
        assert_eq!(
            r.next_after(&LA, utc("2026-10-26T00:00:00Z")),
            Some(utc("2026-11-29T18:00:00Z"))
        );
    }

    #[test]
    fn nth_weekday_counts_from_the_first() {
        // 1 Nov 2026 is a Sunday, so it's the first.
        let r = monthly_nth(2, SUN, "10:00");
        assert_eq!(
            r.next_after(&LA, utc("2026-10-24T00:00:00Z")),
            Some(utc("2026-11-08T18:00:00Z"))
        );
        assert_eq!(
            r.latest_at_or_before(&LA, utc("2026-10-24T00:00:00Z")),
            Some(utc("2026-10-11T17:00:00Z"))
        );
        let r = monthly_nth(1, WED, "10:00");
        assert_eq!(
            r.next_after(&LA, utc("2026-10-24T00:00:00Z")),
            Some(utc("2026-11-04T18:00:00Z"))
        );
    }

    #[test]
    fn monthly_crosses_the_year() {
        let r = monthly_nth(-1, SUN, "10:00");
        assert_eq!(
            r.next_after(&LA, utc("2026-12-28T00:00:00Z")),
            Some(utc("2027-01-31T18:00:00Z"))
        );
        assert_eq!(
            r.latest_at_or_before(&LA, utc("2027-01-05T00:00:00Z")),
            Some(utc("2026-12-27T18:00:00Z"))
        );
        let r = monthly_on(15, "10:00");
        assert_eq!(
            r.next_after(&LA, utc("2026-12-16T00:00:00Z")),
            Some(utc("2027-01-15T18:00:00Z"))
        );
        assert_eq!(
            r.latest_at_or_before(&LA, utc("2027-01-14T00:00:00Z")),
            Some(utc("2026-12-15T18:00:00Z"))
        );
    }

    #[test]
    fn monthly_starts_on_or_after_its_date() {
        let r = Rule::parse(
            ScheduleKind::Monthly,
            Some("2027-03-01"),
            0,
            Some(15),
            None,
            "10:00",
            None,
        )
        .unwrap();
        assert_eq!(
            r.next_after(&LA, utc("2026-10-24T00:00:00Z")),
            Some(utc("2027-03-15T17:00:00Z"))
        );
        assert_eq!(
            r.latest_at_or_before(&LA, utc("2027-03-10T00:00:00Z")),
            None
        );
    }

    #[test]
    fn monthly_skipped_time_moves_to_the_first_minute_after() {
        // 8 Mar 2026: 2:00 PST jumps to 3:00 PDT.
        let r = monthly_on(8, "02:30");
        assert_eq!(
            r.next_after(&LA, utc("2026-02-09T00:00:00Z")),
            Some(utc("2026-03-08T10:00:00Z"))
        );
    }

    #[test]
    fn rejects_bad_monthly_rules() {
        let parse = |days, day, week| {
            Rule::parse(ScheduleKind::Monthly, None, days, day, week, "08:00", None)
        };
        assert!(parse(0, None, None).is_err());
        assert!(parse(SUN, Some(15), Some(2)).is_err());
        assert!(parse(0, Some(0), None).is_err());
        assert!(parse(0, Some(32), None).is_err());
        assert!(parse(SUN, None, Some(5)).is_err());
        assert!(parse(SUN, None, Some(0)).is_err());
        assert!(parse(0, None, Some(2)).is_err());
        assert!(parse(SUN | WED, None, Some(2)).is_err());
        assert!(parse(SUN, None, Some(-1)).is_ok());
        assert!(parse(0, Some(31), None).is_ok());
    }
}
