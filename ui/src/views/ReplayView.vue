<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useStorage } from '@vueuse/core'
import { useViewKeys } from '@/composables/useViewKeys'
import { ChevronLeft, ChevronRight, History, Pause, Play, Rewind, Save, Settings2, Square, X } from '@lucide/vue'
import { useSourcesStore } from '@/stores/sources'
import { useNodesStore } from '@/stores/nodes'
import { usePlayoutStore } from '@/stores/playout'
import { useReplayStore } from '@/stores/replay'
import { clockTimeFine } from '@/lib/format'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import { ToggleGroup, ToggleGroupItem } from '@/components/ui/toggle-group'
import PageHeader from '@/components/common/PageHeader.vue'
import OptionSelect from '@/components/OptionSelect.vue'
import ProgramMonitor from '@/components/playback/ProgramMonitor.vue'
import { useChannelChoice } from '@/composables/useChannelChoice'
import AngleList from '@/components/replay/AngleList.vue'
import AngleLive from '@/components/replay/AngleLive.vue'
import BuffersSheet from '@/components/replay/BuffersSheet.vue'
import ReplayTimeline from '@/components/replay/ReplayTimeline.vue'
import BehindLive from '@/components/replay/BehindLive.vue'
import ClipList from '@/components/replay/ClipList.vue'
import { SPEEDS, useReplayControl } from '@/components/replay/useReplayControl'

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
  [['Shift', 'S'], 'Save in → out on every angle, as one clip'],
  [['Esc'], 'Stop (back to black)'],
]

const { channel, choice: channelChoice } = useChannelChoice('cr.replay.channel')
const channelOptions = computed(() =>
  playout.channels.map((c) => ({
    value: c.key,
    label: nodes.nodes.length > 1 ? `${c.display_name} (${nodes.nameOf(c.node_id)})` : c.display_name,
  })),
)
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
/** The angles that play on the channel: number keys 1–9 pick from these. */
const usableAngles = computed(() =>
  angles.value.filter((a) => !!channel.value && a.buffer.node_id === channel.value.node_id),
)
const angleKey = useStorage('cr.replay.angle', '')
/** The angle a replay on the channel shows, if one does. */
const onAirKey = computed(() => {
  const c = channel.value
  const id = c ? playout.status.get(c.key)?.clip?.replay_source_id : null
  return c && id ? `${c.node_id}/${id}` : null
})
/** The chosen angle; failing that (gone, or on another node), the one on air, else the first. */
const angle = computed(
  () =>
    usableAngles.value.find((a) => a.source.key === angleKey.value) ??
    usableAngles.value.find((a) => a.source.key === onAirKey.value) ??
    usableAngles.value[0] ??
    null,
)

// Loaded once: then `replay.buffers` events keep it, and the store reloads on reconnect.
onMounted(() => replay.loadAll())

const rate = useStorage('cr.replay.rate', 0.5)
const lastSecs = useStorage('cr.replay.last', 8)
/** Seconds the strip reaches back; 0 for the whole buffer (storing null would drop the key, back to the default). */
const windowSecs = useStorage<number>('cr.replay.window', 30)
const windowChoice = computed({
  get: () => (windowSecs.value > 0 ? String(windowSecs.value) : 'all'),
  // Clicking the chosen item again unselects it: keep the choice.
  set: (v: string | undefined) => {
    if (v) windowSecs.value = v === 'all' ? 0 : Number(v)
  },
})
const buffersOpen = ref(false)

const ctl = useReplayControl({ channel, angles, usableAngles, angle, angleKey, clips, rate, lastSecs })
const { status, showing, playing, inUs, outUs, reverse } = ctl

/** The time of day a node clock time shows, from the buffer's UTC mapping. */
function timeOfDay(us: number | null) {
  const offset = angle.value?.buffer.utc_offset_us
  if (us === null || offset == null) return '–'
  return clockTimeFine(new Date((us + offset) / 1000), 2)
}

useViewKeys((e) => {
  const key = e.key.toLowerCase()
  const handled = () => e.preventDefault()
  if (/^[1-9]$/.test(key)) {
    const a = usableAngles.value[Number(key) - 1]
    if (a) ctl.selectAngle(a.source)
  } else if (key === ' ') {
    handled()
    ctl.playPause()
  } else if (key === 'r') ctl.replayLast()
  else if (key === 'arrowleft') {
    handled()
    ctl.step(-1)
  } else if (key === 'arrowright') {
    handled()
    ctl.step(1)
  } else if (key === 'arrowup') {
    handled()
    ctl.nudgeRate(1)
  } else if (key === 'arrowdown') {
    handled()
    ctl.nudgeRate(-1)
  } else if (key === 'j') {
    ctl.setReverse(true)
    ctl.play()
  } else if (key === 'k') ctl.pause()
  else if (key === 'l') {
    ctl.setReverse(false)
    ctl.play()
  } else if (key === 'i') ctl.mark('in')
  else if (key === 'o') ctl.mark('out')
  else if (key === 'enter') {
    handled()
    ctl.playMarked()
  } else if (key === 's') ctl.saveMarked(e.shiftKey)
  else if (key === 'escape') ctl.stop()
}, { alsoIgnore: '[role="listbox"]' })

onMounted(() => sources.ensureLoaded())
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
      <AngleList
        :angles="angles"
        :usable-angles="usableAngles"
        :selected-key="angle?.source.key ?? null"
        :showing-key="showing?.key ?? null"
        @select="ctl.selectAngle"
      />

      <main class="flex-1 min-w-0 overflow-y-auto p-4">
        <div v-if="channel" class="max-w-4xl mx-auto flex flex-col gap-4">
          <div class="grid grid-cols-[2fr_1fr] gap-3 items-start">
            <ProgramMonitor :source="channel" :status="status" />
            <AngleLive v-if="angle" :angle="angle" />
          </div>

          <div
            v-if="status?.error"
            class="rounded-md border border-destructive/40 bg-destructive/10 px-3 py-2 text-xs text-destructive"
          >
            {{ status.error }}
          </div>

          <section class="flex flex-col gap-3">
            <div class="flex flex-wrap items-center gap-2">
              <div class="flex items-center gap-1">
                <Button size="sm" class="h-8 gap-1.5" :disabled="!angle" title="Replay the last few seconds (R)" @click="ctl.replayLast">
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
              <Button size="icon" variant="outline" class="size-8" :disabled="!showing" title="Step back a frame (←)" aria-label="Step back a frame" @click="ctl.step(-1)">
                <ChevronLeft class="size-4" />
              </Button>
              <Button
                v-if="!playing"
                size="sm"
                class="h-8 w-24 gap-1.5"
                :disabled="!angle"
                :title="showing ? 'Play from the frame shown (Space)' : 'Replay the last few seconds (Space)'"
                @click="ctl.play"
              >
                <component :is="rate < 0 ? Rewind : Play" class="size-3.5 fill-current" /> Play
              </Button>
              <Button v-else size="sm" variant="secondary" class="h-8 w-24 gap-1.5" title="Pause (Space)" @click="ctl.pause">
                <Pause class="size-3.5 fill-current" /> Pause
              </Button>
              <Button size="icon" variant="outline" class="size-8" :disabled="!showing" title="Step forward a frame (→)" aria-label="Step forward a frame" @click="ctl.step(1)">
                <ChevronRight class="size-4" />
              </Button>
              <Button size="sm" variant="outline" class="h-8 gap-1.5" :disabled="!status?.clip" title="Stop: back to black (Esc)" @click="ctl.stop">
                <Square class="size-3 fill-current" /> Stop
              </Button>
              <div class="w-px h-6 bg-border mx-1" />
              <Button
                size="sm"
                :variant="reverse ? 'default' : 'outline'"
                class="h-8 gap-1.5 text-xs"
                :aria-pressed="reverse"
                title="Play backwards (J backwards, L forwards)"
                @click="ctl.setReverse(!reverse)"
              >
                <Rewind class="size-3.5" /> Reverse
              </Button>
              <ToggleGroup
                :model-value="String(Math.abs(rate))"
                type="single"
                variant="segmented"
                aria-label="Speed"
                @update:model-value="(v) => v && ctl.setRate((reverse ? -1 : 1) * Number(v))"
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
                  <BehindLive
                    class="text-lg"
                    :buffer="angle?.buffer ?? null"
                    :showing-us="showing?.us ?? null"
                    :playing-rate="playing ? (showing?.rate ?? null) : null"
                  />
                </span>
              </div>
            </div>

            <ReplayTimeline
              v-if="angle"
              :buffer="angle.buffer"
              :window-secs="windowSecs > 0 ? windowSecs : null"
              :playhead-us="showing?.us ?? null"
              :playhead-other="!!showing && showing.key !== angle.source.key"
              :playing="playing && !!showing"
              :in-us="inUs"
              :out-us="outUs"
              @pick="ctl.pick"
            />

            <div class="flex flex-wrap items-center gap-2 text-xs">
              <Button size="sm" variant="outline" class="h-7 text-xs" :disabled="!showing" title="Mark in at the frame shown (I)" @click="ctl.mark('in')">
                Mark in
              </Button>
              <span class="num w-24">{{ inUs === null ? 'In –' : `In ${timeOfDay(inUs)}` }}</span>
              <Button size="sm" variant="outline" class="h-7 text-xs" :disabled="!showing" title="Mark out at the frame shown (O)" @click="ctl.mark('out')">
                Mark out
              </Button>
              <span class="num w-24">{{ outUs === null ? 'Out –' : `Out ${timeOfDay(outUs)}` }}</span>
              <Button
                size="sm"
                class="h-7 text-xs gap-1.5"
                :disabled="inUs === null"
                title="Play from in to out at the chosen speed (Enter)"
                @click="ctl.playMarked"
              >
                <Play class="size-3 fill-current" /> Play in → out
              </Button>
              <Button
                size="sm"
                variant="outline"
                class="h-7 text-xs gap-1.5"
                :disabled="inUs === null || outUs === null || outUs <= inUs"
                title="Save in → out as a clip (S; Shift+S on every angle)"
                @click="ctl.saveMarked(false)"
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
                @click="ctl.clearMarks"
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
            :angle-source-id="angle?.source.id ?? null"
            @cue="ctl.cueClip"
            @play="ctl.playClip"
          />
        </div>
      </main>
    </div>
  </div>

  <BuffersSheet v-if="buffersOpen" @close="buffersOpen = false" />
</template>
