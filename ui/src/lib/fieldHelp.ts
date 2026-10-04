/**
 * Explanations behind the `?` beside each preset field. Kept together so the
 * wording stays consistent; each entry says what the field does, what the
 * options mean when that isn't obvious, and what the default is and why.
 */
export type FieldHelp = {
  title: string
  body: string
  options?: { label: string; text: string }[]
  default?: string
}

export const FIELD_HELP = {
  outputName: {
    title: 'Output name',
    body: 'A label for this output. It shows in the editor and the Recordings tab, and fills {output} in the path template, so it can tell outputs apart, e.g. "Master" and "Proxy".',
  },
  codec: {
    title: 'Codec',
    body: 'How the video is compressed. It decides file size, picture quality, how hard the node works, and which apps can open the file.',
    options: [
      { label: 'H.264', text: 'Plays everywhere, with small files and good quality.' },
      { label: 'H.265 / HEVC', text: 'About 40% smaller than H.264 at the same quality, but heavier to encode and not every player supports it.' },
      { label: 'VP9', text: 'An open codec about as efficient as H.265. .mkv only, and few editing apps open it.' },
      { label: 'ProRes Proxy / LT', text: "Apple's editing codec at low data rates, for offline editing and proxies." },
      { label: 'ProRes 422 / HQ', text: 'Large files that scrub and cut smoothly. 422 suits most finishing; HQ is for the highest quality.' },
      { label: 'ProRes 4444', text: 'Full color detail with alpha, for graphics and keying.' },
      { label: 'Uncompressed', text: 'Raw frames, about 1 Gbps at 1080p30. Only when nothing may be lost.' },
    ],
    default: 'H.264, the most compatible choice.',
  },
  container: {
    title: 'Container',
    body: "The file type that holds the video and audio. Not every codec fits every container; options that don't fit are greyed out.",
    options: [
      { label: '.mov', text: 'QuickTime. Best for editing apps and the usual home of ProRes.' },
      { label: '.mp4', text: 'The most widely playable. H.264 and H.265 only.' },
      { label: '.mkv', text: 'Matroska. Takes any codec and holds up well if recording is cut off, but fewer editing apps open it.' },
    ],
    default: '.mov',
  },
  resolution: {
    title: 'Resolution',
    body: "The frame size of the recording. Any size other than the source's scales the video, which costs some processing. A different aspect ratio gets black bars rather than being stretched.",
    options: [
      { label: 'Match source', text: 'Records exactly what the source sends, with no scaling.' },
      { label: 'Custom…', text: 'Any width and height.' },
    ],
    default: 'Match source',
  },
  framerate: {
    title: 'Frame rate',
    body: 'Frames per second in the file. Choosing a rate other than the source\'s drops or repeats frames to reach it, e.g. for a lower-rate proxy or to keep every file at one rate.',
    options: [
      { label: 'Match source', text: 'Keeps every frame the source sends.' },
      { label: '23.976 · 29.97 · 59.94', text: 'NTSC rates (North America, Japan), exactly 24000/1001, 30000/1001 and 60000/1001.' },
      { label: '25 · 50', text: 'PAL rates (Europe and most of the world).' },
      { label: '24 · 30 · 60', text: 'Film, web and computer rates.' },
      { label: 'Custom…', text: 'Any number or fraction, e.g. 12.5 or 30000/1001.' },
    ],
    default: 'Match source',
  },
  bitrate: {
    title: 'Bitrate',
    body: 'How much data per second the video may use, in kilobits per second. Higher looks better and makes bigger files: 1,000 kbps is about 450 MB per hour. Simple scenes may use less.',
    options: [
      { label: 'Blank (Auto)', text: 'Scales with the frame size and rate: about 12,000 kbps for H.264 at 1080p30, and less for H.265 and VP9, which compress better.' },
    ],
    default: 'Auto',
  },
  bitrateQuality: {
    title: 'Bitrate',
    body: 'In Constant quality mode (Advanced) the size follows the content, so there is no target bitrate. Switch Rate control back to Average or Constant bitrate to set one.',
  },
  bitrateFixed: {
    title: 'Bitrate',
    body: 'ProRes and uncompressed video have a data rate fixed by the codec and its profile, so there is nothing to set. Pick a lighter ProRes profile for smaller files.',
  },
  chroma: {
    title: 'Chroma subsampling',
    body: 'How much color detail is kept. Brightness is always stored in full; color can be stored at lower resolution, which the eye barely notices in most footage.',
    options: [
      { label: '4:2:0', text: 'Color at quarter resolution. Plays everywhere and can use hardware encoding.' },
      { label: '4:2:2', text: 'Color at half resolution. The broadcast standard, with cleaner edges for keying and graphics. Encodes in software, and some players can\'t play it.' },
      { label: '4:4:4', text: 'Full color resolution, for screen recordings and graphics. Encodes in software, with limited playback.' },
    ],
    default: '4:2:0',
  },
  chromaFixed: {
    title: 'Chroma subsampling',
    body: 'Set by the codec: ProRes 422 variants are 4:2:2, ProRes 4444 is 4:4:4, and uncompressed video keeps what the source sends.',
  },
  pathTemplate: {
    title: 'Path template',
    body: "Where each recording is written on the node that records it, and what it's called. The {tokens} are filled in when recording starts; click one below to insert it. ~ is the node's home folder, and missing folders are created.",
    options: [
      { label: '{take}', text: 'Use it to never overwrite: it counts up until the file name is free.' },
      { label: '{output}', text: 'Needed when several outputs would otherwise get the same file name.' },
      { label: '{segment}', text: 'The file number when an output splits (Advanced). Added before the extension if the template leaves it out.' },
    ],
  },

  // ── Advanced ──────────────────────────────────────────────────────────────

  encoder: {
    title: 'Encoder',
    body: 'Whether this output is encoded by a dedicated video chip (hardware) or by the CPU (software). Hardware barely loads the node, so many outputs fit at once; software works on every node, produces the same result everywhere, and offers more control.',
    options: [
      { label: 'Auto', text: "Hardware where the node has it, otherwise software. Each node decides for itself." },
      { label: 'Hardware', text: "Only hardware. A node without it can't record this output. H.264/H.265 hardware takes 4:2:0 only." },
      { label: 'Software', text: 'Always the CPU, for identical results on every node, or 4:2:2/4:4:4.' },
    ],
    default: 'Auto',
  },
  encoderFixed: {
    title: 'Encoder',
    body: 'Uncompressed video is written as it arrives, with no encoder.',
  },
  rateControl: {
    title: 'Rate control',
    body: 'How the encoder spends data across the recording.',
    options: [
      { label: 'Average bitrate', text: 'Aims for the Bitrate on average, giving complex scenes more and simple ones less. Predictable file size with good quality. Hardware encoders treat the average loosely and can run about 1.5× over on grainy footage.' },
      { label: 'Constant bitrate', text: 'Holds the Bitrate every second, padding simple scenes. Exactly predictable size; complex scenes can look worse. For streaming-style delivery or strict storage budgets.' },
      { label: 'Constant quality', text: 'Holds the picture quality steady and lets the size follow the content: a still wide shot is tiny, confetti is huge. Best quality for the space used, but storage is hard to predict.' },
    ],
    default: 'Average bitrate',
  },
  rateControlFixed: {
    title: 'Rate control',
    body: 'ProRes and uncompressed video have a data rate set by the codec, so there is nothing to control.',
  },
  quality: {
    title: 'Quality',
    body: 'How good the picture should look in Constant quality mode, from 1 to 100. Higher keeps more detail and makes bigger files. Each encoder maps this onto its own scale.',
    options: [
      { label: '50–60', text: 'Proxies and review copies.' },
      { label: '70', text: 'Good-looking masters at a sensible size.' },
      { label: '85+', text: 'Visually close to lossless; large files.' },
    ],
    default: '70',
  },
  speedPreset: {
    title: 'Speed preset',
    body: 'How hard the x264/x265 software encoders work to compress. Slower presets look better at the same bitrate (or make smaller files at the same quality), but each step uses roughly 1.5–2× the CPU. Too slow for the node and frames get dropped — watch the dropped-frame count.',
    options: [
      { label: 'ultrafast', text: 'Least CPU; noticeably bigger files or blockier picture.' },
      { label: 'veryfast', text: 'A good balance for live recording.' },
      { label: 'medium – slow', text: 'Best compression; only with CPU to spare, and few outputs.' },
    ],
    default: 'veryfast',
  },
  speedPresetFixed: {
    title: 'Speed preset',
    body: 'Only the x264 and x265 software encoders have speed presets. Hardware encoders run at a fixed speed, and VP9 uses its own realtime settings.',
  },
  keyframes: {
    title: 'Keyframe interval',
    body: 'Seconds between keyframes, the complete pictures that other frames are stored as changes from. Shorter makes scrubbing and cutting faster and more accurate and limits damage from corruption, at the cost of slightly bigger files.',
    options: [
      { label: '1–2 s', text: 'Editing and file splitting.' },
      { label: '4–10 s', text: 'Smaller files for archive or delivery.' },
    ],
    default: '2 seconds',
  },
  keyframesFixed: {
    title: 'Keyframe interval',
    body: 'Every ProRes and uncompressed frame is a complete picture, so there is no interval to set.',
  },
  deinterlace: {
    title: 'Deinterlace',
    body: 'Interlaced video (1080i, common from broadcast sources) stores each frame as two half-pictures taken at different moments. Deinterlacing turns them into whole frames, avoiding comb-like edges on motion.',
    options: [
      { label: 'Auto', text: 'Deinterlaces interlaced sources; progressive video passes through untouched.' },
      { label: 'Off', text: 'Records interlaced video as it arrives, e.g. to deinterlace later in the edit.' },
    ],
    default: 'Auto',
  },
  deinterlaceFixed: {
    title: 'Deinterlace',
    body: 'ProRes stores interlaced video natively, so it is recorded as it arrives.',
  },
  audioCodec: {
    title: 'Audio codec',
    body: 'How the audio is stored.',
    options: [
      { label: 'Auto', text: 'PCM beside ProRes and uncompressed video, AAC in .mov/.mp4, and Opus in .mkv.' },
      { label: 'PCM 24-bit', text: 'Uncompressed, about 1.15 Mbps per channel. No quality loss; what editors expect. .mov or .mkv.' },
      { label: 'AAC', text: 'Compressed and plays everywhere.' },
      { label: 'Opus', text: 'Better than AAC at the same bitrate, but fewer editing apps take it. .mkv only.' },
    ],
    default: 'Auto',
  },
  audioBitrate: {
    title: 'Audio bitrate',
    body: 'Data rate of compressed (AAC or Opus) audio, in kilobits per second. Higher sounds cleaner on music and complex mixes.',
    options: [
      { label: 'Blank', text: '256 kbps for AAC, 160 kbps for Opus: transparent for program audio.' },
      { label: '96–128', text: 'Speech and review copies.' },
    ],
  },
  audioBitrateFixed: {
    title: 'Audio bitrate',
    body: 'PCM is uncompressed: its data rate follows the channel count, about 1.15 Mbps per channel.',
  },
  channels: {
    title: 'Channels',
    body: 'Which audio channels to record. NDI sources often carry several, e.g. a stereo mix plus isolated mics.',
    options: [
      { label: 'All', text: 'Every channel the source sends.' },
      { label: 'Stereo mix', text: 'Mixed down to two. Surround layouts are downmixed properly; plain numbered channels go odd-left, even-right.' },
      { label: 'Pick…', text: 'Only the channels listed, in that order, e.g. "1-2" for the program mix or "3, 4". Channels the source lacks are silent. AAC and Opus take 1 or 2.' },
    ],
    default: 'All',
  },
  splitEvery: {
    title: 'Split every',
    body: 'Start a new file at regular intervals instead of recording one long file. Each file plays on its own, files follow on with no frames lost or repeated between them, and a crash or full disk can only affect the file being written. Files are numbered with {segment} (001, 002, …).',
    options: [
      { label: 'Off', text: 'One file for the whole recording.' },
      { label: '15–60 min', text: 'Easier to copy, back up and start editing while recording continues.' },
    ],
    default: 'Off. Outputs with PCM audio in .mov (ProRes and uncompressed, by default) still start a new file every 4 hours, which keeps them crash-safe; most recordings never reach it.',
  },
  splitSize: {
    title: 'Split at size',
    body: 'Start a new file when the current one reaches about this size, e.g. to stay under a drive\'s or upload service\'s file size limit. With a time as well, whichever comes first. Files split at a keyframe, so they can run slightly over.',
    options: [{ label: 'Blank', text: 'No size limit.' }],
  },
} satisfies Record<string, FieldHelp>
