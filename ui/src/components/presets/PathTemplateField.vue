<script setup lang="ts">
import { computed, ref, watch } from 'vue'
import { ChevronDown } from '@lucide/vue'
import type { PresetOutputInput } from '@/types/generated/PresetOutputInput'
import {
  EXT_SUFFIX,
  PATH_PATTERNS,
  PATH_TOKEN_GROUPS,
  applyPattern,
  expandPath,
  joinTemplate,
  splitTemplate,
  withExtToken,
} from '@/lib/codecs'
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu'
import HelpTip from '@/components/common/HelpTip.vue'
import { FIELD_HELP } from '@/lib/fieldHelp'
import TokenInput from '@/components/presets/TokenInput.vue'

/**
 * Where an output's files go, edited as a folder and a file name; the
 * extension always comes from the container. Stored as one `path_template`.
 * A template that doesn't end in `.{ext}` opens as one field, as does any
 * template after "Edit as one path".
 */
const props = defineProps<{
  /** Another output would write the same file. */
  clash: boolean
  problem?: string
  multipleOutputs: boolean
  /** Example values for the preview. */
  preview: { source: string; sourceName: string; node: string; preset: string }
}>()
const leg = defineModel<PresetOutputInput>({ required: true })

const template = computed({
  get: () => leg.value.path_template,
  set: (path_template: string) => (leg.value = { ...leg.value, path_template }),
})

const single = ref(splitTemplate(template.value).name === null)

// The two fields keep what's typed (a folder ending in `/` mid-edit) and are
// refreshed only when the template changes some other way.
const folder = ref('')
const name = ref('')
watch(
  template,
  (t) => {
    const s = splitTemplate(t)
    if (s.name === null || joinTemplate(folder.value, name.value) === t) return
    folder.value = s.folder
    name.value = s.name
  },
  { immediate: true },
)

function commit() {
  template.value = joinTemplate(folder.value, name.value)
}

function setFolder(v: string) {
  // A whole template pasted into the folder.
  if (v.endsWith(EXT_SUFFIX)) {
    const s = splitTemplate(v)
    folder.value = s.folder
    name.value = s.name!
  } else folder.value = v
  commit()
}

function setName(v: string) {
  if (v.endsWith(EXT_SUFFIX)) v = v.slice(0, -EXT_SUFFIX.length)
  // Folders typed or pasted into the name move to the folder field.
  const slash = v.lastIndexOf('/')
  if (slash >= 0) {
    const dir = v.slice(0, slash)
    const base = folder.value.replace(/\/+$/, '')
    folder.value = /^[/~]/.test(v) ? dir || '/' : base ? `${base}/${dir}` : dir
    v = v.slice(slash + 1)
  }
  name.value = v
  commit()
}

function split() {
  template.value = withExtToken(template.value)
  single.value = false
}

const extWillChange = computed(() => single.value && splitTemplate(template.value).name === null)

/** Every token but `{ext}`, which the split editor adds itself. */
const splitGroups = PATH_TOKEN_GROUPS.map((g) => ({ ...g, tokens: g.tokens.filter((t) => t.token !== '{ext}') }))

function usePattern(path: string) {
  template.value = applyPattern(template.value, path, props.multipleOutputs)
  single.value = false
}

const previewPath = computed(() => expandPath(leg.value, props.preview))
const invalid = computed(() => !!props.problem || props.clash)
</script>

<template>
  <div class="flex flex-col gap-1.5">
    <div class="flex items-center gap-1 text-xs text-muted-foreground">
      Save to <HelpTip :help="FIELD_HELP.pathTemplate" />
      <span class="opacity-60 truncate">· ~ is the recording node's home</span>
      <div class="ml-auto flex items-center gap-3 shrink-0">
        <DropdownMenu>
          <DropdownMenuTrigger class="flex items-center gap-0.5 hover:text-foreground">
            Patterns <ChevronDown class="size-3" />
          </DropdownMenuTrigger>
          <DropdownMenuContent align="end" class="w-80">
            <DropdownMenuLabel class="text-[11px] font-normal text-muted-foreground">Keeps your root folder</DropdownMenuLabel>
            <DropdownMenuItem v-for="p in PATH_PATTERNS" :key="p.label" class="flex-col items-start gap-0" @select="usePattern(p.path)">
              <span>{{ p.label }}</span>
              <span class="num text-[11px] text-muted-foreground">{{ p.path }}{{ multipleOutputs ? '_{output}' : '' }}{{ EXT_SUFFIX }}</span>
            </DropdownMenuItem>
          </DropdownMenuContent>
        </DropdownMenu>
        <button v-if="single" type="button" class="hover:text-foreground" @click="split">Split into folder and name</button>
        <button v-else type="button" class="hover:text-foreground" @click="single = true">Edit as one path</button>
      </div>
    </div>

    <template v-if="single">
      <TokenInput v-model="template" :groups="PATH_TOKEN_GROUPS" :invalid="invalid" aria-label="Path template" />
      <span v-if="extWillChange" class="text-[11px] text-muted-foreground">
        This path doesn't end in {{ EXT_SUFFIX }}. Splitting it sets the extension from the container.
      </span>
    </template>
    <div v-else class="grid gap-2 lg:grid-cols-2">
      <label class="flex flex-col gap-1 min-w-0">
        <span class="text-[11px] text-muted-foreground">Folder</span>
        <TokenInput
          :model-value="folder"
          :groups="splitGroups"
          :invalid="invalid"
          placeholder="~/capture-room/{date}"
          aria-label="Folder"
          @update:model-value="setFolder"
        />
      </label>
      <label class="flex flex-col gap-1 min-w-0">
        <span class="text-[11px] text-muted-foreground">File name</span>
        <TokenInput
          :model-value="name"
          :groups="splitGroups"
          :invalid="invalid"
          :suffix="EXT_SUFFIX"
          placeholder="{source}_{time}"
          aria-label="File name"
          @update:model-value="setName"
        />
      </label>
    </div>

    <div class="text-xs flex gap-2 min-w-0">
      <span class="text-muted-foreground shrink-0">Preview</span>
      <span class="num break-all">{{ previewPath }}</span>
    </div>
    <span v-if="problem" class="text-[11px] text-destructive">{{ problem }}</span>
    <span v-if="clash" class="text-[11px] text-destructive">
      Another output writes the same file — vary the path, container, or include {output}.
    </span>
  </div>
</template>
