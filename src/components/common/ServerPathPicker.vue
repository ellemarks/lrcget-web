<template>
  <BaseModal
    :model-value="!!request"
    :title="title"
    content-class="w-full max-w-xl h-full max-h-[80vh]"
    body-class="flex flex-col h-full gap-3"
    @close="cancel"
    @update:model-value="value => !value && cancel()"
  >
    <form class="flex-none flex gap-2" @submit.prevent="navigate(pathInput)">
      <button
        type="button"
        class="flex-none button button-normal p-2 rounded-full"
        :disabled="!listing?.parent"
        title="Parent folder"
        @click="navigate(listing.parent)"
      >
        <ArrowUp />
      </button>
      <input v-model="pathInput" type="text" class="grow input px-3 py-2 font-mono text-sm" />
    </form>

    <div v-if="error" class="flex-none text-sm text-red-700 dark:text-red-400">{{ error }}</div>

    <div v-if="loading" class="grow flex justify-center items-center">
      <Loading class="animate-spin text-2xl text-neutral-800 dark:text-neutral-600" />
    </div>

    <div v-else class="grow overflow-y-auto flex flex-col gap-0.5 min-h-0">
      <div
        v-if="listing && listing.entries.length === 0"
        class="py-8 text-center text-sm text-neutral-500"
      >
        {{ isDirectoryMode ? 'No subfolders' : 'No matching files' }}
      </div>
      <button
        v-for="entry in listing?.entries ?? []"
        :key="entry.path"
        class="w-full text-left px-2 py-1.5 rounded transition flex items-center gap-2 text-sm"
        :class="[
          selectedFile === entry.path
            ? 'bg-hoa-1100 text-white dark:bg-hoa-1100'
            : 'text-neutral-800 dark:text-neutral-300 hover:bg-neutral-100 dark:hover:bg-neutral-800',
        ]"
        @click="entry.isDir ? navigate(entry.path) : (selectedFile = entry.path)"
        @dblclick="!entry.isDir && choose(entry.path)"
      >
        <Folder v-if="entry.isDir" class="flex-none text-neutral-500" />
        <FileOutline v-else class="flex-none text-neutral-500" />
        <span class="truncate">{{ entry.name }}</span>
      </button>
    </div>

    <template #footer>
      <div class="flex gap-2">
        <button class="button button-normal px-6 py-2 rounded-full" @click="cancel">Cancel</button>
        <button
          v-if="isDirectoryMode"
          class="button button-primary px-6 py-2 rounded-full"
          :disabled="!listing"
          @click="choose(listing.path)"
        >
          Use this folder
        </button>
        <button
          v-else
          class="button px-6 py-2 rounded-full"
          :class="selectedFile ? 'button-primary' : 'button-disabled'"
          :disabled="!selectedFile"
          @click="choose(selectedFile)"
        >
          Open
        </button>
      </div>
    </template>
  </BaseModal>
</template>

<script setup>
// Browser-mode replacement for the native file/folder dialog: browses the server's filesystem.
import { computed, ref, watch } from 'vue'
import ArrowUp from '~icons/mdi/arrow-up'
import Folder from '~icons/mdi/folder'
import FileOutline from '~icons/mdi/file-outline'
import Loading from '~icons/mdi/loading'
import { getServerInfo } from '@/platform'
import { pathPickerRequest as request, finishPathPick } from '@/platform/path-picker.js'

const LAST_DIRECTORY_KEY = 'lrcget.pathPicker.lastDirectory'

const listing = ref(null)
const pathInput = ref('')
const selectedFile = ref(null)
const loading = ref(false)
const error = ref(null)

const isDirectoryMode = computed(() => !!request.value?.options?.directory)
const title = computed(
  () =>
    request.value?.options?.title ?? (isDirectoryMode.value ? 'Choose a folder' : 'Choose a file')
)
const extensions = computed(() =>
  (request.value?.options?.filters ?? [])
    .flatMap(filter => filter.extensions)
    .filter(ext => ext !== '*')
)

const readLastDirectory = () => {
  try {
    return localStorage.getItem(LAST_DIRECTORY_KEY)
  } catch {
    return null
  }
}

const rememberDirectory = path => {
  try {
    localStorage.setItem(LAST_DIRECTORY_KEY, path)
  } catch {
    // Not important enough to report.
  }
}

const fetchListing = async path => {
  const params = new URLSearchParams()
  if (path) {
    params.set('path', path)
  }
  if (!isDirectoryMode.value) {
    params.set('files', 'true')
    params.set('extensions', extensions.value.join(','))
  }
  const response = await fetch(new URL(`api/fs?${params}`, document.baseURI))
  const body = await response.json()
  if (!response.ok) {
    throw body
  }
  return body
}

const navigate = async path => {
  loading.value = true
  error.value = null
  try {
    listing.value = await fetchListing(path)
    pathInput.value = listing.value.path
    selectedFile.value = null
    rememberDirectory(listing.value.path)
  } catch (err) {
    error.value = err?.toString?.() ?? 'Cannot open folder'
    pathInput.value = listing.value?.path ?? path ?? ''
  } finally {
    loading.value = false
  }
}

watch(request, async current => {
  if (!current) {
    return
  }
  listing.value = null
  selectedFile.value = null
  const info = await getServerInfo().catch(() => ({}))
  const candidates = [current.options?.defaultPath, readLastDirectory(), info.musicDir, null]
  for (const candidate of candidates) {
    await navigate(candidate)
    if (listing.value) {
      error.value = null
      break
    }
  }
})

const choose = path => finishPathPick(path)
const cancel = () => finishPathPick(null)
</script>
