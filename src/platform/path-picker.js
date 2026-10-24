import { shallowRef } from 'vue'

// The pending request rendered by `ServerPathPicker.vue`: `{ options, resolve }` or null.
export const pathPickerRequest = shallowRef(null)

/** Opens the server-side picker; resolves to the chosen path, or null when cancelled. */
export function pickServerPath(options) {
  return new Promise(resolve => {
    pathPickerRequest.value?.resolve(null)
    pathPickerRequest.value = { options, resolve }
  })
}

export function finishPathPick(path) {
  const request = pathPickerRequest.value
  pathPickerRequest.value = null
  request?.resolve(path)
}
