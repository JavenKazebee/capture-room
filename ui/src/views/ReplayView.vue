<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useIntervalFn, useStorage } from '@vueuse/core'
import { useViewKeys } from '@/composables/useViewKeys'
import {
  ChevronLeft,
  ChevronRight,
  History,
  Pause,
  Play,
  Rewind,
  Save,
  Settings2,
  Square,
  X,
} from '@lucide/vue'
import { useSourcesStore, type Source } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import { useReplayStore, type ReplayBuffer, type ReplayClip } from '@/stores/replay'
import { notifyError } from '@/lib/notify'
import { clockTimeFine, formatClock } from '@/lib/format'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import PageHeader from '@/components/common/PageHeader.vue'
import StatusDot from '@/components/common/StatusDot.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import ProgramMonitor from '@/components/playback/ProgramMonitor.vue'
import BuffersSheet from '@/components/replay/BuffersSheet.vue'
import ReplayTimeline from '@/components/replay/ReplayTimeline.vue'
import BehindLive from '@/components/replay/BehindLive.vue'
import ClipList from '@/components/replay/ClipList.vue'
import { useThumbnail } from '@/components/record/useThumbnail'

const sources = useSourcesStore()
const nodes = useNodesStore()
const playout = usePlayoutStore()
const replay = useReplayStore()

const SHORTCUTS: [string[], string][] = [
  [['R'], 'Replay the last few seconds'],
  [['1', '…', '9'], 'Switch angle, at the same moment'],
  [['Space'], 'Play / pause'],
  [['←', '→'], 'Step a frame back / forward'],
  [['J', 'K', 'L'], 'Play backwards / pause / play forwards'],
  [['↑', '↓'], 'Faster / slower'],
  [['I', 'O'], 'Mark in / out at the frame shown'],
  [['Enter'], 'Play from in to out'],
  [['S'], 'Save in → out as a clip'],
  [['Shift', 'S'], 'Save in → out on every angle'],
  [['Esc'], 'Stop (back to black)'],
]

/** Speeds offered, slowest first; either way round. */
const SPEEDS = [0.25, 0.5, 0.75, 1] as const

// ── Channel and angles ────────────────────────────────────────────────────────

const channelKey = useStorage('cr.replay.channel', '')
const channel = computed(
  () => playout.channels.find((c) => c.key === channelKey.value) ?? playout.channels[0] ?? null,
)
const channelOptions = computed(() =>
  playout.channels.map((c) => ({
    value: c.key,
    label: nodes.nodes.length > 1 ? `${c.display_name} (${nodes.nameOf(c.node_id)})` : c.display_name,
  })),
)
/** The picker shows the channel in use, also before one's been picked. */
const channelChoice = computed({
  get: () => channel.value?.key ?? '',
  set: (key: string) => (channelKey.value = key),
})
const status = computed(() => (channel.value ? playout.status.get(channel.value.key) : undefined))
watch(
  channel,
  (c) => {
    if (c) playout.loadStatus(c.node_id, c.id).catch(() => {})
    if (c) replay.loadClips(c.node_id).catch(() => {})
  },
  { immediate: true },
)

/** Saved clips on the channel's node: they play on its channels. */
const clips = computed(() => (channel.value ? (replay.clips.get(channel.value.node_id) ?? []) : []))

/** Sources with a buffer, in source order. A buffer plays on its own node's channels. */
const angles = computed(() =>
  sources.sources
    .filter((s) => replay.buffers.has(s.key))
    .map((s) => ({ source: s, buffer: replay.buffers.get(s.key)! })),
)
const usable = (b: ReplayBuffer) => !!channel.value && b.node_id === channel.value.node_id
/** The angles that play on the channel: number keys 1–9 pick from these. */
const usableAngles = computed(() => angles.value.filter((a) => usable(a.buffer)))
const angleKey = useStorage('cr.replay.angle', '')
const angle = computed(
  () =>
    angles.value.find((a) => a.source.key === angleKey.value && usable(a.buffer)) ??
    angles.value.find((a) => usable(a.buffer)) ??
    null,
)

// ── Time ──────────────────────────────────────────────────────────────────────

// One poll at a time: a slow node mustn't pile requests up.
let polling = false
useIntervalFn(
  async () => {
    if (polling) return
    polling = true
    try {
      await replay.loadAll()
    } finally {
      polling = false
    }
  },
  1000,
  { immediateCallback: true },
)

/** The replay on the channel, if it's showing one: its angle and the moment shown. */
const showing = computed(() => {
  const clip = status.value?.clip
  if (!clip?.replay_source_id || clip.replay_start_us == null || !channel.value) return null
  const pos = status.value?.position_ms ?? clip.in_ms
  return {
    key: `${channel.value.node_id}/${clip.replay_source_id}`,
    us: clip.replay_start_us + pos * 1000,
    rate: clip.rate,
  }
})
const playing = computed(() => status.value?.state === 'playing')

/** The time of day a node clock time shows, from the buffer's UTC mapping. */
function timeOfDay(us: number | null) {
  const offset = angle.value?.buffer.utc_offset_us
  if (us === null || offset == null) return '–'
  return clockTimeFine(new Date((us + offset) / 1000), 2)
}

// ── Settings ──────────────────────────────────────────────────────────────────

const rate = useStorage('cr.replay.rate', 0.5)
const lastSecs = useStorage('cr.replay.last', 8)
const windowSecs = useStorage<number | null>('cr.replay.window', 30)
const windowChoice = computed({
  get: () => String(windowSecs.value ?? 'all'),
  set: (v: string) => (windowSecs.value = v === 'all' ? null : Number(v)),
})
const inUs = ref<number | null>(null)
const outUs = ref<number | null>(null)
const buffersOpen = ref(false)

// ── Commands ──────────────────────────────────────────────────────────────────

/**
 * Commands run one after another, in the order given: a key pressed while
 * one is in flight (a step, a cue) waits its turn rather than being lost.
 */
let queue: Promise<unknown> = Promise.resolve()
/** Commands queued or running. */
const queued = ref(0)
const busyQueue = computed(() => queued.value > 0)
function run(what: string, fn: () => Promise<unknown>) {
  queued.value++
  queue = queue
    .then(fn)
    .catch((e) => notifyError(`${what} failed`, e, channel.value?.node_id ?? null))
    .finally(() => queued.value--)
}

/** Put `source`'s buffer on air from `fromUs`, up to the out mark if it's later. */
function load(source: Source, fromUs: number, play: boolean, r = rate.value) {
  const c = channel.value
  if (!c) return Promise.resolve()
  const to = outUs.value !== null && outUs.value > fromUs ? outUs.value : null
  return replay.play(c.node_id, c.id, {
    source_id: source.id,
    from_us: Math.round(fromUs),
    to_us: to === null ? null : Math.round(to),
    rate: r,
    play,
  })
}

// Each command decides what to do when its turn comes, from the state
// the commands before it left, not from the state when its key was pressed.

/** Show a moment of the current angle, paused. */
function pick(us: number) {
  const a = angle.value
  if (a) run('Cueing', () => load(a.source, us, false))
}

/** The classic: the last few seconds again, at the chosen speed. */
function replayLast() {
  const a = angle.value
  if (!a) return
  // From the moment the key was pressed.
  const from = replay.nowUs(a.buffer) - Math.max(1, Number(lastSecs.value) || 8) * 1_000_000
  outUs.value = null
  run('Replaying', () => load(a.source, from, true))
}

function playMarked() {
  const a = angle.value
  if (!a) return
  run('Playing the marked clip', async () => {
    if (inUs.value !== null) await load(a.source, inUs.value, true)
  })
}

/** Switch angle; a replay on air carries on from the same moment on the new one. */
function selectAngle(source: Source) {
  const b = replay.buffers.get(source.key)
  if (!b || !usable(b)) return
  angleKey.value = source.key
  run(`Switching to ${source.display_name}`, async () => {
    const s = showing.value
    if (s && s.key !== source.key) await load(source, s.us, playing.value, s.rate)
  })
}

/** Play from the frame shown at the chosen speed; with nothing shown, the last few seconds. */
function play() {
  const c = channel.value
  if (!c) return
  if (!showing.value && !busyQueue.value) return replayLast()
  run('Playing', async () => {
    const s = showing.value
    const clip = status.value?.clip
    if (!s || !clip || playing.value) return
    if (status.value?.state === 'ended') {
      // Again from its in point (its out point backwards).
      const saved = clips.value.find((x) => x.id === clip.replay_clip_id)
      if (saved) return replay.playClip(c.id, saved, rate.value, true)
      const a = angles.value.find((x) => x.source.key === s.key)
      if (!a) return
      const start = clip.replay_start_us! + clip.in_ms * 1000
      const end = clip.out_ms == null ? null : clip.replay_start_us! + clip.out_ms * 1000
      return replay.play(c.node_id, c.id, {
        source_id: a.source.id,
        from_us: Math.round(start),
        to_us: end === null ? null : Math.round(end),
        rate: rate.value,
        play: true,
      })
    }
    if (s.rate !== rate.value) await playout.transport(c.node_id, c.id, 'speed', { rate: rate.value })
    await playout.transport(c.node_id, c.id, 'play')
  })
}

function pause() {
  const c = channel.value
  if (!c) return
  run('Pausing', async () => {
    if (playing.value) await playout.transport(c.node_id, c.id, 'pause')
  })
}

function playPause() {
  // Decided now: a Space pressed while a replay is starting pauses it.
  if (playing.value || busyQueue.value) {
    run('Pausing', async () => {
      const c = channel.value
      if (c && playing.value) await playout.transport(c.node_id, c.id, 'pause')
      else if (c && showing.value) await playout.transport(c.node_id, c.id, 'play')
    })
  } else play()
}

function stop() {
  const c = channel.value
  if (!c) return
  run('Stopping', async () => {
    if (status.value?.clip) await playout.transport(c.node_id, c.id, 'stop')
  })
}

function step(frames: number) {
  const c = channel.value
  if (!c) return
  run('Stepping', async () => {
    if (showing.value) await playout.transport(c.node_id, c.id, 'step', { frames })
  })
}

function setRate(r: number) {
  rate.value = r
  const c = channel.value
  if (!c) return
  run('Changing speed', async () => {
    if (showing.value && playing.value && showing.value.rate !== r) await playout.transport(c.node_id, c.id, 'speed', { rate: r })
  })
}

function nudgeRate(dir: 1 | -1) {
  const i = SPEEDS.indexOf(Math.abs(rate.value) as (typeof SPEEDS)[number])
  const next = SPEEDS[Math.min(SPEEDS.length - 1, Math.max(0, (i < 0 ? 1 : i) + dir))]
  setRate(rate.value < 0 ? -next : next)
}

const reverse = computed(() => rate.value < 0)
function setReverse(on: boolean) {
  setRate(on ? -Math.abs(rate.value) : Math.abs(rate.value))
}

/** Save in → out as a clip of the selected angle, or of every angle on the channel's node. */
function saveMarked(everyAngle: boolean) {
  const c = channel.value
  const a = angle.value
  if (!c || !a) return
  run('Saving the clip', async () => {
    const from = inUs.value
    const to = outUs.value
    if (from === null || to === null || to <= from) throw new Error('Mark an in point and a later out point first.')
    const targets = everyAngle ? usableAngles.value : [a]
    for (const t of targets) {
      await replay.saveClip(c.node_id, {
        source_id: t.source.id,
        from_us: Math.round(from),
        to_us: Math.round(to),
        name: null,
      })
    }
  })
}

function cueClip(clip: ReplayClip) {
  const c = channel.value
  if (c) run(`Cueing ${clip.name}`, () => replay.playClip(c.id, clip, rate.value, false))
}

function playClip(clip: ReplayClip) {
  const c = channel.value
  if (c) run(`Playing ${clip.name}`, () => replay.playClip(c.id, clip, rate.value, true))
}

/** Mark the frame shown, once the commands before have shown it. */
function mark(which: 'in' | 'out') {
  run('Marking', async () => {
    const us = showing.value?.us ?? null
    if (us === null) return
    if (which === 'in') inUs.value = us
    else outUs.value = us
  })
}

// ── Keyboard ──────────────────────────────────────────────────────────────────

useViewKeys((e) => {
  const key = e.key.toLowerCase()
  const handled = () => e.preventDefault()
  if (/^[1-9]$/.test(key)) {
    const a = usableAngles.value[Number(key) - 1]
    if (a) selectAngle(a.source)
  } else if (key === ' ') {
    handled()
    playPause()
  } else if (key === 'r') replayLast()
  else if (key === 'arrowleft') {
    handled()
    step(-1)
  } else if (key === 'arrowright') {
    handled()
    step(1)
  } else if (key === 'arrowup') {
    handled()
    nudgeRate(1)
  } else if (key === 'arrowdown') {
    handled()
    nudgeRate(-1)
  } else if (key === 'j') {
    setReverse(true)
    play()
  } else if (key === 'k') pause()
  else if (key === 'l') {
    setReverse(false)
    play()
  } else if (key === 'i') mark('in')
  else if (key === 'o') mark('out')
  else if (key === 'enter') {
    handled()
    playMarked()
  } else if (key === 's') saveMarked(e.shiftKey)
  else if (key === 'escape') stop()
}, { alsoIgnore: '[role="listbox"]' })

onMounted(() => sources.ensureLoaded())

// ── Angle tiles ───────────────────────────────────────────────────────────────

/** Held span of a buffer, as "4:32". */
function held(b: ReplayBuffer) {
  return b.start_us !== null && b.end_us !== null ? formatClock((b.end_us - b.start_us) / 1000) : '0:00.0'
}

const angleSource = computed(() => angle.value?.source ?? null)
const angleThumb = useThumbnail(angleSource)
</script>

<template>
  <div class="h-full flex flex-col min-h-0">
    <PageHeader title="Replay" :count="angles.length" :shortcuts="SHORTCUTS">
      <template #actions>
        <Button size="sm" variant="outline" class="h-7 gap-1.5 text-xs" @click="buffersOpen = true">
          <Settings2 class="size-3.5" /> Buffers
        </Button>
      </template>
      <div v-if="playout.channels.length" class="flex items-center gap-2 text-xs">
        <span class="text-muted-foreground">Out on</span>
        <OptionSelect v-model="channelChoice" :options="channelOptions" class="h-7 w-48 text-xs" />
      </div>
    </PageHeader>

    <div v-if="!angles.length || !playout.channels.length" class="flex-1 grid place-items-center p-8">
      <div class="max-w-sm text-center text-sm text-muted-foreground flex flex-col items-center gap-3">
        <template v-if="!angles.length">
          <p>
            No replay buffers yet. Give a source one and its node keeps the last few minutes of it, ready to play
            back at any speed on a playout channel.
          </p>
          <Button size="sm" class="gap-1.5" @click="buffersOpen = true">
            <History class="size-3.5" /> Set up replay buffers
          </Button>
        </template>
        <p v-else>Replays play out on a playout channel: add one in Playback first.</p>
      </div>
    </div>

    <div v-else class="flex-1 min-h-0 flex">
      <!-- Angles -->
      <nav class="w-60 shrink-0 border-r border-border overflow-y-auto p-2 flex flex-col gap-2" aria-label="Angles">
        <button
          v-for="a in angles"
          :key="a.source.key"
          class="text-left rounded-md border p-1.5 flex flex-col gap-1"
          :class="[
            a.source.key === angle?.source.key ? 'border-primary ring-1 ring-primary' : 'border-border hover:bg-accent/40',
            !usable(a.buffer) && 'opacity-50 cursor-not-allowed',
            showing?.key === a.source.key && 'bg-success/10',
          ]"
          :disabled="!usable(a.buffer)"
          :title="usable(a.buffer) ? undefined : `On ${nodes.nameOf(a.buffer.node_id)}: it plays on that node's channels`"
          :aria-current="a.source.key === angle?.source.key || undefined"
          @click="selectAngle(a.source)"
        >
          <span class="flex items-center gap-1.5 text-xs min-w-0">
            <span class="num text-[10px] rounded bg-muted px-1 text-muted-foreground shrink-0">
              {{ usable(a.buffer) ? usableAngles.indexOf(a) + 1 : '–' }}
            </span>
            <StatusDot :status="a.buffer.error ? 'error' : a.buffer.encoder ? 'ok' : 'off'" />
            <span class="font-medium truncate">{{ a.source.display_name }}</span>
            <span class="flex-1" />
            <span class="num text-[10px] text-muted-foreground shrink-0">{{ held(a.buffer) }}</span>
          </span>
          <span v-if="a.buffer.error" class="text-[11px] text-destructive line-clamp-2">{{ a.buffer.error }}</span>
        </button>
      </nav>

      <main class="flex-1 min-w-0 overflow-y-auto p-4">
        <div v-if="channel" class="max-w-4xl mx-auto flex flex-col gap-4">
          <div class="grid grid-cols-[2fr_1fr] gap-3 items-start">
            <ProgramMonitor :source="channel" :status="status" />
            <!-- The selected angle, live -->
            <div v-if="angle" class="rounded-lg border border-border overflow-hidden bg-card">
              <div class="relative aspect-video bg-black">
                <img
                  v-if="angleThumb.src.value"
                  :src="angleThumb.src.value"
                  :alt="`${angle.source.display_name} live`"
                  class="w-full h-full object-contain"
                  draggable="false"
                  @error="angleThumb.onError"
                />
                <span class="absolute top-1.5 left-1.5 rounded-sm bg-black/70 px-1.5 py-0.5 text-[10px] font-semibold uppercase tracking-wide text-zinc-300">
                  Live
                </span>
              </div>
              <div class="px-2 py-1.5 text-xs flex flex-col gap-0.5">
                <span class="font-medium truncate">{{ angle.source.display_name }}</span>
                <span class="text-muted-foreground num">{{ held(angle.buffer) }} held · {{ angle.buffer.encoder ?? 'not recording' }}</span>
              </div>
            </div>
          </div>

          <div
            v-if="status?.error"
            class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
          >
            {{ status.error }}
          </div>

          <!-- Transport -->
          <section class="flex flex-col gap-3">
            <div class="flex flex-wrap items-center gap-2">
              <div class="flex items-center gap-1">
                <Button size="sm" class="h-8 gap-1.5" :disabled="!angle" title="Replay the last few seconds (R)" @click="replayLast">
                  <History class="size-3.5" /> Last
                </Button>
                <Input
                  v-model.number="lastSecs"
                  type="number"
                  min="1"
                  class="h-8 w-14 num"
                  aria-label="Seconds to replay"
                />
                <span class="text-xs text-muted-foreground">s</span>
              </div>
              <div class="w-px h-6 bg-border mx-1" />
              <Button size="icon" variant="outline" class="size-8" :disabled="!showing" title="Step back a frame (←)" aria-label="Step back a frame" @click="step(-1)">
                <ChevronLeft class="size-4" />
              </Button>
              <Button
                v-if="!playing"
                size="sm"
                class="h-8 w-24 gap-1.5"
                :disabled="!angle"
                :title="showing ? 'Play from the frame shown (Space)' : 'Replay the last few seconds (Space)'"
                @click="play"
              >
                <component :is="rate < 0 ? Rewind : Play" class="size-3.5 fill-current" /> Play
              </Button>
              <Button v-else size="sm" variant="secondary" class="h-8 w-24 gap-1.5" title="Pause (Space)" @click="pause">
                <Pause class="size-3.5 fill-current" /> Pause
              </Button>
              <Button size="icon" variant="outline" class="size-8" :disabled="!showing" title="Step forward a frame (→)" aria-label="Step forward a frame" @click="step(1)">
                <ChevronRight class="size-4" />
              </Button>
              <Button size="sm" variant="outline" class="h-8 gap-1.5" :disabled="!status?.clip" title="Stop: back to black (Esc)" @click="stop">
                <Square class="size-3 fill-current" /> Stop
              </Button>
              <div class="w-px h-6 bg-border mx-1" />
              <Button
                size="sm"
                :variant="reverse ? 'default' : 'outline'"
                class="h-8 gap-1.5 text-xs"
                :aria-pressed="reverse"
                title="Play backwards (J backwards, L forwards)"
                @click="setReverse(!reverse)"
              >
                <Rewind class="size-3.5" /> Reverse
              </Button>
              <ToggleGroup
                :model-value="String(Math.abs(rate))"
                type="single"
                variant="segmented"
                aria-label="Speed"
                @update:model-value="(v) => v && setRate((reverse ? -1 : 1) * Number(v))"
              >
                <ToggleGroupItem v-for="s in SPEEDS" :key="s" :value="String(s)" class="h-8 px-2.5 text-xs" :title="'↑ faster, ↓ slower'">
                  {{ Math.round(s * 100) }}%
                </ToggleGroupItem>
              </ToggleGroup>
              <div class="flex-1" />
              <div class="flex items-baseline gap-4 num">
                <span class="flex flex-col items-end leading-tight">
                  <span class="text-[10px] uppercase tracking-wide text-muted-foreground font-sans">Showing</span>
                  <span class="text-lg">{{ timeOfDay(showing?.us ?? null) }}</span>
                </span>
                <span class="flex flex-col items-end leading-tight">
                  <span class="text-[10px] uppercase tracking-wide text-muted-foreground font-sans">Behind live</span>
                  <BehindLive class="text-lg" :buffer="angle?.buffer ?? null" :showing-us="showing?.us ?? null" />
                </span>
              </div>
            </div>

            <ReplayTimeline
              v-if="angle"
              :buffer="angle.buffer"
              :window-secs="windowSecs"
              :playhead-us="showing?.key === angle.source.key ? showing.us : null"
              :in-us="inUs"
              :out-us="outUs"
              @pick="pick"
            />

            <div class="flex flex-wrap items-center gap-2 text-xs">
              <Button size="sm" variant="outline" class="h-7 text-xs" :disabled="!showing" title="Mark in at the frame shown (I)" @click="mark('in')">
                Mark in
              </Button>
              <span class="num w-24">{{ inUs === null ? 'In –' : `In ${timeOfDay(inUs)}` }}</span>
              <Button size="sm" variant="outline" class="h-7 text-xs" :disabled="!showing" title="Mark out at the frame shown (O)" @click="mark('out')">
                Mark out
              </Button>
              <span class="num w-24">{{ outUs === null ? 'Out –' : `Out ${timeOfDay(outUs)}` }}</span>
              <Button
                size="sm"
                class="h-7 text-xs gap-1.5"
                :disabled="inUs === null"
                title="Play from in to out at the chosen speed (Enter)"
                @click="playMarked"
              >
                <Play class="size-3 fill-current" /> Play in → out
              </Button>
              <Button
                size="sm"
                variant="outline"
                class="h-7 text-xs gap-1.5"
                :disabled="inUs === null || outUs === null || outUs <= inUs"
                title="Save in → out as a clip (S; Shift+S on every angle)"
                @click="saveMarked(false)"
              >
                <Save class="size-3" /> Save clip
              </Button>
              <Button
                v-if="inUs !== null || outUs !== null"
                size="icon"
                variant="ghost"
                class="size-7"
                title="Clear the marks"
                aria-label="Clear the marks"
                @click="(inUs = null), (outUs = null)"
              >
                <X class="size-3.5" />
              </Button>
              <div class="flex-1" />
              <span class="text-muted-foreground">Show</span>
              <ToggleGroup v-model="windowChoice" type="single" variant="segmented" aria-label="Timeline span">
                <ToggleGroupItem value="30" class="h-7 px-2 text-xs">30 s</ToggleGroupItem>
                <ToggleGroupItem value="120" class="h-7 px-2 text-xs">2 min</ToggleGroupItem>
                <ToggleGroupItem value="all" class="h-7 px-2 text-xs">All</ToggleGroupItem>
              </ToggleGroup>
            </div>
          </section>

          <ClipList
            :clips="clips"
            :on-air-id="status?.clip?.replay_clip_id ?? null"
            @cue="cueClip"
            @play="playClip"
          />
        </div>
      </main>
    </div>
  </div>

  <BuffersSheet v-if="buffersOpen" @close="buffersOpen = false" />
</template>
