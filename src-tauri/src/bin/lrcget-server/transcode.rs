//! Fallback for audio the browser cannot decode itself (e.g. ALAC in Chrome/Firefox): decode it
//! with Symphonia, the same decoder the desktop player uses, into a cached 16-bit WAV file.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::{DecoderOptions, CODEC_TYPE_NULL};
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use tokio::sync::Mutex;

/// Transcoded files to keep; older ones are removed.
const CACHE_SIZE: usize = 10;

/// One transcode at a time: the browser often requests the same file several times in a row.
static TRANSCODE_LOCK: Mutex<()> = Mutex::const_new(());

/// Returns a WAV version of `source`, decoding it on first use.
pub async fn to_wav(source: PathBuf, cache_dir: PathBuf) -> Result<PathBuf, String> {
    let _guard = TRANSCODE_LOCK.lock().await;
    tokio::task::spawn_blocking(move || {
        let target = cache_dir.join(format!("{}.wav", cache_key(&source)?));
        if !target.is_file() {
            std::fs::create_dir_all(&cache_dir).map_err(|err| err.to_string())?;
            let partial = target.with_extension("partial");
            if let Err(err) = decode_to_wav(&source, &partial) {
                let _ = std::fs::remove_file(&partial);
                return Err(err);
            }
            std::fs::rename(&partial, &target).map_err(|err| err.to_string())?;
            prune_cache(&cache_dir);
        }
        Ok(target)
    })
    .await
    .map_err(|err| err.to_string())?
}

/// Changes whenever the source file is replaced or modified.
fn cache_key(source: &Path) -> Result<String, String> {
    let metadata = std::fs::metadata(source).map_err(|err| err.to_string())?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or_default();
    let identity = format!("{}\0{}\0{}", source.display(), metadata.len(), modified);
    Ok(format!(
        "{:016x}",
        xxhash_rust::xxh3::xxh3_64(identity.as_bytes())
    ))
}

fn prune_cache(cache_dir: &Path) {
    let Ok(read_dir) = std::fs::read_dir(cache_dir) else {
        return;
    };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = read_dir
        .filter_map(Result::ok)
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "wav"))
        .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files.into_iter().skip(CACHE_SIZE) {
        let _ = std::fs::remove_file(path);
    }
}

fn decode_to_wav(source: &Path, target: &Path) -> Result<(), String> {
    let file = File::open(source).map_err(|err| err.to_string())?;
    let stream = MediaSourceStream::new(Box::new(file), Default::default());
    let mut hint = Hint::new();
    if let Some(extension) = source.extension().and_then(|ext| ext.to_str()) {
        hint.with_extension(extension);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|err| err.to_string())?;
    let mut format = probed.format;
    let track = format
        .tracks()
        .iter()
        .find(|track| track.codec_params.codec != CODEC_TYPE_NULL)
        .ok_or("No audio track found")?;
    let track_id = track.id;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|err| err.to_string())?;

    let mut writer = BufWriter::new(File::create(target).map_err(|err| err.to_string())?);
    // Placeholder header, rewritten once the length is known.
    writer.write_all(&[0; 44]).map_err(|err| err.to_string())?;

    let mut samples: Option<SampleBuffer<i16>> = None;
    let mut sample_rate = 44_100;
    let mut channels = 2;
    let mut data_len: u64 = 0;

    loop {
        let packet = match format.next_packet() {
            Ok(packet) => packet,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(SymphoniaError::ResetRequired) => break,
            Err(err) => return Err(err.to_string()),
        };
        if packet.track_id() != track_id {
            continue;
        }
        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            // Skip corrupt packets, like the desktop player does.
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(err) => return Err(err.to_string()),
        };

        let spec = *decoded.spec();
        sample_rate = spec.rate;
        channels = spec.channels.count() as u16;
        let buffer = match samples.as_mut() {
            Some(buffer) if buffer.capacity() >= decoded.capacity() * spec.channels.count() => {
                buffer
            }
            _ => samples.insert(SampleBuffer::new(decoded.capacity() as u64, spec)),
        };
        buffer.copy_interleaved_ref(decoded);

        let mut bytes = Vec::with_capacity(buffer.samples().len() * 2);
        for sample in buffer.samples() {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        writer.write_all(&bytes).map_err(|err| err.to_string())?;
        data_len += bytes.len() as u64;
    }

    if data_len > u32::MAX as u64 - 36 {
        return Err("Track is too long to transcode".to_owned());
    }
    writer
        .seek(SeekFrom::Start(0))
        .and_then(|_| writer.write_all(&wav_header(sample_rate, channels, data_len as u32)))
        .and_then(|_| writer.flush())
        .map_err(|err| err.to_string())
}

fn wav_header(sample_rate: u32, channels: u16, data_len: u32) -> [u8; 44] {
    let bits_per_sample: u16 = 16;
    let block_align = channels * bits_per_sample / 8;
    let byte_rate = sample_rate * block_align as u32;

    let mut header = [0u8; 44];
    header[0..4].copy_from_slice(b"RIFF");
    header[4..8].copy_from_slice(&(36 + data_len).to_le_bytes());
    header[8..12].copy_from_slice(b"WAVE");
    header[12..16].copy_from_slice(b"fmt ");
    header[16..20].copy_from_slice(&16u32.to_le_bytes());
    header[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    header[22..24].copy_from_slice(&channels.to_le_bytes());
    header[24..28].copy_from_slice(&sample_rate.to_le_bytes());
    header[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    header[32..34].copy_from_slice(&block_align.to_le_bytes());
    header[34..36].copy_from_slice(&bits_per_sample.to_le_bytes());
    header[36..40].copy_from_slice(b"data");
    header[40..44].copy_from_slice(&data_len.to_le_bytes());
    header
}
