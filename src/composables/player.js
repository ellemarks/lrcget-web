import { ref } from 'vue'
import { useToast } from 'vue-toastification'
import { audioUrl, invoke, isTauri, listen } from '@/platform'

const playingTrack = ref(null)
const status = ref('stopped')
const duration = ref(null)
const progress = ref(null)
const volume = ref(1.0)
const playbackSpeed = ref(1.0)

// Desktop: the Rust backend plays audio and reports its state every 40ms.
listen('player-state', async event => {
  duration.value = event.payload.duration
  progress.value = event.payload.progress
  status.value = event.payload.status
  volume.value = event.payload.volume
  playbackSpeed.value = event.payload.playback_speed ?? 1.0
})

listen('reload-track-id', async event => {
  const payload = event.payload
  if (playingTrack.value && playingTrack.value.id === payload) {
    playingTrack.value = await invoke('get_track', { trackId: playingTrack.value.id })
  }
})

// Browser: an <audio> element streams the file from the server and drives the same state.
const browserPlayer = (() => {
  let audio = null
  let ticker = null
  let transcoding = false

  const syncState = () => {
    progress.value = audio.currentTime
    duration.value = Number.isFinite(audio.duration)
      ? audio.duration
      : (playingTrack.value?.duration ?? 0)
    status.value = audio.ended ? 'stopped' : audio.paused ? 'paused' : 'playing'

    // `timeupdate` fires only every ~250ms, too coarse for synced lyrics.
    if (status.value === 'playing' && !ticker) {
      ticker = setInterval(syncState, 40)
    } else if (status.value !== 'playing' && ticker) {
      clearInterval(ticker)
      ticker = null
    }
  }

  const startPlayback = () => {
    audio.play().catch(error => {
      if (error.name !== 'AbortError') {
        console.error('Playback failed', error)
      }
    })
  }

  // Formats the browser can't decode (e.g. ALAC) are retried as WAV decoded by the server.
  const handleError = () => {
    // Clearing the source on stop is not an error worth reporting.
    if (!audio.getAttribute('src')) {
      syncState()
      return
    }
    const track = playingTrack.value
    const code = audio.error?.code
    const unsupported =
      code === MediaError.MEDIA_ERR_SRC_NOT_SUPPORTED || code === MediaError.MEDIA_ERR_DECODE
    if (track && unsupported && !transcoding) {
      transcoding = true
      const position = audio.currentTime
      audio.src = audioUrl(track, { transcode: true })
      audio.currentTime = position
      startPlayback()
      return
    }
    useToast().error(`Cannot play ${track?.title ?? 'this track'} in the browser`)
    syncState()
  }

  const element = () => {
    if (!audio) {
      audio = new Audio()
      audio.preload = 'auto'
      audio.volume = Math.min(Math.max(volume.value, 0), 1)
      for (const event of ['play', 'pause', 'ended', 'seeked', 'timeupdate', 'durationchange']) {
        audio.addEventListener(event, syncState)
      }
      audio.addEventListener('error', handleError)
    }
    return audio
  }

  return {
    play(track) {
      const audio = element()
      transcoding = false
      audio.src = audioUrl(track)
      audio.playbackRate = playbackSpeed.value
      startPlayback()
    },
    pause() {
      element().pause()
    },
    resume() {
      startPlayback()
    },
    // Like the desktop player, seeking also starts playback, reloading the track after stop().
    seek(position) {
      const audio = element()
      if (!audio.getAttribute('src')) {
        this.play(playingTrack.value)
      }
      audio.currentTime = position
      if (audio.paused) {
        startPlayback()
      }
      syncState()
    },
    stop() {
      const audio = element()
      audio.pause()
      audio.removeAttribute('src')
      audio.load()
      syncState()
      progress.value = 0
      duration.value = 0
      status.value = 'stopped'
    },
    setVolume(value) {
      element().volume = Math.min(Math.max(value, 0), 1)
      volume.value = value
    },
    setPlaybackSpeed(value) {
      const speed = Math.min(Math.max(value, 0.5), 2.0)
      element().playbackRate = speed
      playbackSpeed.value = speed
    },
  }
})()

export function usePlayer() {
  const playTrack = track => {
    playingTrack.value = track

    if (!isTauri) {
      browserPlayer.play(track)
      return
    }

    // Determine if this is a database track or a file-based track
    if (track.id !== undefined && track.id !== null) {
      // Database track - use track_id
      invoke('play_track', {
        trackId: track.id,
        filePath: null,
        title: track.title,
        albumName: track.album_name,
        artistName: track.artist_name,
        albumArtistName: track.album_artist_name,
        duration: track.duration,
      })
    } else if (track.file_path) {
      // File-based track (from file picker) - use file_path with metadata
      invoke('play_track', {
        trackId: null,
        filePath: track.file_path,
        title: track.title,
        albumName: track.album_name,
        artistName: track.artist_name,
        albumArtistName: track.album_artist_name,
        duration: track.duration,
      })
    }
  }

  const pause = () => {
    if (!playingTrack.value) {
      return
    }

    if (!isTauri) {
      browserPlayer.pause()
      return
    }

    invoke('pause_track')
  }

  const resume = () => {
    if (!playingTrack.value) {
      return
    }

    if (!isTauri) {
      browserPlayer.resume()
      return
    }

    invoke('resume_track')
  }

  const seek = position => {
    if (!playingTrack.value) {
      return
    }

    if (!isTauri) {
      browserPlayer.seek(position)
      return
    }

    if (status.value === 'stopped') {
      invoke('play_track', { trackId: playingTrack.value.id })
    }

    invoke('seek_track', { position })
  }

  const stop = () => {
    if (!playingTrack.value) {
      return
    }

    if (!isTauri) {
      browserPlayer.stop()
      return
    }

    invoke('stop_track')
  }

  const setVolume = volume => {
    if (!isTauri) {
      browserPlayer.setVolume(volume)
    }
    // Also persists the volume in the config
    invoke('set_volume', { volume })
  }

  const setPlaybackSpeed = playbackSpeedValue => {
    if (!isTauri) {
      browserPlayer.setPlaybackSpeed(playbackSpeedValue)
      return
    }

    invoke('set_playback_speed', { playbackSpeed: playbackSpeedValue })
  }

  return {
    playingTrack,
    status,
    duration,
    progress,
    volume,
    playbackSpeed,
    playTrack,
    pause,
    resume,
    stop,
    seek,
    setVolume,
    setPlaybackSpeed,
  }
}
