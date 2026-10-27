import { computed, ref, type Ref } from 'vue'
import { recordingFileUrl } from '@/composables/useApi'
import { fileName } from '@/lib/format'
import { outputFiles, previewOutput, type RecordingSession } from '@/stores/recordings'

/** The player beside a recording: which file it shows, and playing or picking one. */
export function useRecordingPlayer(session: Ref<RecordingSession>, outputs: Ref<{ name: string; files: string[] }[]>) {
  const live = computed(() => session.value.status === 'active')

  /** The file playing, by path, since deleting a file shifts the indexes after it. */
  const playing = ref<{ output: number; path: string } | null>(initialPlaying())
  function initialPlaying() {
    const output = previewOutput(session.value)
    const path = output === -1 ? undefined : outputFiles(session.value, output)[0]
    return path === undefined ? null : { output, path }
  }
  /** The playing file's index in its output, `-1` once it's been deleted. */
  const playingFile = computed(() =>
    playing.value ? (outputs.value[playing.value.output]?.files.indexOf(playing.value.path) ?? -1) : -1,
  )
  const playingDeleted = computed(() => !!playing.value && playingFile.value === -1)
  const playError = ref(false)
  const video = ref<HTMLVideoElement>()
  /** Starts playback as soon as a file loads — only once someone picks a file, not on open. */
  const autoplay = ref(false)

  const playingUrl = computed(() =>
    playing.value && !live.value && playingFile.value !== -1
      ? recordingFileUrl(session.value.node_id, session.value.id, playing.value.output, playingFile.value)
      : null,
  )

  /** What the player shows, for the caption under it. */
  const playingCaption = computed(() => {
    if (!playingUrl.value || !playing.value) return null
    const o = outputs.value[playing.value.output]
    if (!o) return null
    return {
      output: o.name,
      file: fileName(playing.value.path),
      part: o.files.length > 1 ? `${playingFile.value + 1} of ${o.files.length}` : null,
    }
  })

  const canPlay = (output: number) => !live.value && !!session.value.outputs[output]?.playable

  const isPlaying = (output: number, file: number) =>
    !live.value && playing.value?.output === output && playingFile.value === file

  function play(output: number, file: number) {
    if (!canPlay(output)) return
    if (isPlaying(output, file) && video.value && !playError.value) {
      // Already loaded: play it from the start.
      video.value.currentTime = 0
      void video.value.play()
      return
    }
    const path = outputs.value[output]?.files[file]
    if (path === undefined) return
    playing.value = { output, path }
    playError.value = false
    autoplay.value = true
  }

  /** Plays or pauses the loaded file, or starts the preview file when nothing's loaded yet. */
  function togglePlay() {
    if (video.value && !playError.value) {
      if (video.value.paused) void video.value.play()
      else video.value.pause()
      return
    }
    const output = previewOutput(session.value)
    if (output !== -1) play(output, 0)
  }

  return { video, playError, autoplay, playingUrl, playingCaption, playingDeleted, canPlay, isPlaying, play, togglePlay }
}
