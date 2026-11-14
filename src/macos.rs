//! macOS counterparts of the Windows-only pieces: system-audio capture through
//! ScreenCaptureKit, text recognition through the Vision framework, and the
//! capacity figures that Windows reads from DXGI and GlobalMemoryStatusEx.
//!
//! Audio is captured only while a [`SystemAudio`] value is alive; no
//! microphone is opened. Our own process is excluded from the captured mix,
//! so read-aloud playback never reaches speech recognition.

use anyhow::{Context, Result, bail, ensure};
use block2::RcBlock;
use objc2::{
    AnyThread, DefinedClass, define_class, msg_send,
    rc::Retained,
    runtime::{NSObject, NSObjectProtocol, ProtocolObject},
};
use objc2_core_media::{CMSampleBuffer, CMTime};
use objc2_foundation::{NSArray, NSError, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamOutput,
    SCStreamOutputType,
};
use std::{
    ptr::NonNull,
    sync::{
        Mutex,
        mpsc::{Receiver, SyncSender, TrySendError, sync_channel},
    },
    time::Duration,
};

/// The rate every capture path hands to speech recognition.
pub const RATE: u32 = 16_000;

/// One block of the system mix, already mono and 16 kHz.
pub struct AudioPacket {
    pub samples: Vec<f32>,
    /// Position of the first sample on the capture clock, in 16 kHz samples.
    pub clock_sample: i64,
}

struct OutputIvars {
    tx: SyncSender<AudioPacket>,
    converter: Mutex<Converter>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and the class implements
    // no Drop. The ivars are only touched from ScreenCaptureKit's serial queue
    // and the Mutex protects the converter if Apple ever delivers concurrently.
    #[unsafe(super(NSObject))]
    #[name = "TalkStyleLiveTranslationAudioOutput"]
    #[ivars = OutputIvars]
    struct AudioOutput;

    unsafe impl NSObjectProtocol for AudioOutput {}

    unsafe impl SCStreamOutput for AudioOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        #[allow(non_snake_case)]
        unsafe fn stream_didOutputSampleBuffer_ofType(
            &self,
            _stream: &SCStream,
            sample_buffer: &CMSampleBuffer,
            kind: SCStreamOutputType,
        ) {
            if kind != SCStreamOutputType::Audio {
                return;
            }
            // Never unwind into Objective-C.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let Some(packet) = (unsafe { self.packet(sample_buffer) }) else {
                    return;
                };
                match self.ivars().tx.try_send(packet) {
                    Ok(()) | Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {}
                }
            }));
        }
    }
);

impl AudioOutput {
    fn new(tx: SyncSender<AudioPacket>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(OutputIvars {
            tx,
            converter: Mutex::new(Converter::default()),
        });
        unsafe { msg_send![super(this), init] }
    }

    /// Copies one CoreMedia audio buffer into mono 16 kHz samples.
    unsafe fn packet(&self, buffer: &CMSampleBuffer) -> Option<AudioPacket> {
        use objc2_core_media::CMAudioFormatDescriptionGetStreamBasicDescription;
        if !unsafe { buffer.is_valid() } || !unsafe { buffer.data_is_ready() } {
            return None;
        }
        let frames = usize::try_from(unsafe { buffer.num_samples() }).ok()?;
        if frames == 0 {
            return None;
        }
        let format = unsafe { buffer.format_description() }?;
        let description = unsafe { CMAudioFormatDescriptionGetStreamBasicDescription(&format) };
        let description = unsafe { description.as_ref() }?;
        // kAudioFormatLinearPCM with 32-bit float samples is what ScreenCaptureKit
        // delivers. Anything else is skipped rather than misread as noise.
        const LINEAR_PCM: u32 = u32::from_be_bytes(*b"lpcm");
        const FLOAT: u32 = 1;
        const NON_INTERLEAVED: u32 = 1 << 5;
        if description.mFormatID != LINEAR_PCM
            || description.mFormatFlags & FLOAT == 0
            || description.mBitsPerChannel != 32
        {
            return None;
        }
        let channels = (description.mChannelsPerFrame as usize).max(1);
        let rate = description.mSampleRate;
        let block = unsafe { buffer.data_buffer() }?;
        let length = unsafe { block.data_length() };
        if length < frames * channels * 4 || length > 64 * 1024 * 1024 {
            return None;
        }
        let mut raw = vec![0f32; length / 4];
        let status = unsafe {
            block.copy_data_bytes(0, raw.len() * 4, NonNull::new(raw.as_mut_ptr().cast())?)
        };
        if status != 0 {
            return None;
        }
        let mono: Vec<f32> = if channels == 1 {
            raw.truncate(frames);
            raw
        } else if description.mFormatFlags & NON_INTERLEAVED != 0 {
            (0..frames)
                .map(|i| (0..channels).map(|c| raw[c * frames + i]).sum::<f32>() / channels as f32)
                .collect()
        } else {
            raw.chunks_exact(channels)
                .take(frames)
                .map(|frame| frame.iter().sum::<f32>() / channels as f32)
                .collect()
        };
        let seconds = unsafe { buffer.presentation_time_stamp().seconds() };
        let mut converter = self.ivars().converter.lock().ok()?;
        let samples = converter.convert(&mono, rate);
        if samples.is_empty() {
            return None;
        }
        Some(AudioPacket {
            samples,
            clock_sample: if seconds.is_finite() {
                (seconds * RATE as f64).round() as i64
            } else {
                i64::MIN
            },
        })
    }
}

/// Linear resampler that keeps its phase across packets. ScreenCaptureKit
/// honours the requested 16 kHz almost always; this only guards the rest.
#[derive(Default)]
struct Converter {
    rate: f64,
    position: f64,
    last: f32,
}

impl Converter {
    fn convert(&mut self, input: &[f32], rate: f64) -> Vec<f32> {
        let clean = |x: f32| {
            if x.is_finite() {
                x.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        };
        if !(rate.is_finite() && rate >= 4_000.0) || (rate - RATE as f64).abs() < 0.5 {
            return input.iter().map(|&x| clean(x)).collect();
        }
        if (self.rate - rate).abs() > 0.5 {
            *self = Self {
                rate,
                position: 0.0,
                last: 0.0,
            };
        }
        let step = rate / RATE as f64;
        let mut output = Vec::with_capacity((input.len() as f64 / step) as usize + 2);
        // `position` is measured from the sample before `input[0]` (`last`).
        while self.position < input.len() as f64 {
            let index = self.position.floor() as usize;
            let fraction = (self.position - index as f64) as f32;
            let before = if index == 0 {
                self.last
            } else {
                input[index - 1]
            };
            let after = input[index];
            output.push(clean(before + (after - before) * fraction));
            self.position += step;
        }
        self.position -= input.len() as f64;
        self.last = input.last().copied().unwrap_or(self.last);
        output
    }
}

/// A running ScreenCaptureKit stream. Dropping it stops the capture.
pub struct SystemAudio {
    stream: Retained<SCStream>,
    _output: Retained<AudioOutput>,
    pub packets: Receiver<AudioPacket>,
}

// SAFETY: SCStream is documented as usable from any thread; the output object
// is only retained here and called by ScreenCaptureKit on its own queue.
unsafe impl Send for SystemAudio {}

const PERMISSION_HELP: &str = "PC音声を取得できません。「システム設定」→「プライバシーとセキュリティ」→「画面収録とシステムオーディオ録音」で、このアプリ（Talk風Live翻訳）をオンにしてから、アプリを開き直してください。";

fn error_text(error: *mut NSError) -> Option<String> {
    let error = unsafe { error.as_ref() }?;
    Some(error.localizedDescription().to_string())
}

fn wait<T>(rx: Receiver<T>, what: &str) -> Result<T> {
    rx.recv_timeout(Duration::from_secs(15))
        .with_context(|| format!("{what}の応答がありませんでした。"))
}

impl SystemAudio {
    /// Starts capturing the mix of every app except this one (macOS 13 or later).
    pub fn start() -> Result<Self> {
        let (tx, rx) = sync_channel::<Result<Retained<SCShareableContent>, String>>(1);
        let handler = RcBlock::new(
            move |content: *mut SCShareableContent, error: *mut NSError| {
                let result = match unsafe { Retained::retain(content) } {
                    Some(content) => Ok(content),
                    None => Err(error_text(error).unwrap_or_default()),
                };
                let _ = tx.try_send(result);
            },
        );
        unsafe {
            SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(
                false, true, &handler,
            )
        };
        let content = wait(rx, "画面収録の許可")?
            .map_err(|error| anyhow::anyhow!("{PERMISSION_HELP}（{error}）"))?;
        let displays = unsafe { content.displays() };
        let Some(display) = displays.firstObject() else {
            bail!("音声の取得に使うディスプレイが見つかりません。");
        };
        let filter = unsafe {
            SCContentFilter::initWithDisplay_excludingWindows(
                SCContentFilter::alloc(),
                &display,
                &NSArray::new(),
            )
        };
        let config = unsafe { SCStreamConfiguration::new() };
        unsafe {
            config.setCapturesAudio(true);
            config.setSampleRate(RATE as isize);
            config.setChannelCount(1);
            config.setExcludesCurrentProcessAudio(true);
            // Video cannot be turned off; keep it as small and rare as allowed.
            config.setWidth(2);
            config.setHeight(2);
            config.setMinimumFrameInterval(CMTime::new(1, 1));
            config.setQueueDepth(3);
        }
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(
                SCStream::alloc(),
                &filter,
                &config,
                None,
            )
        };
        let (packet_tx, packets) = sync_channel(256);
        let output = AudioOutput::new(packet_tx);
        let queue = dispatch2::DispatchQueue::new(
            "jp.talk-style-live-translation.audio",
            dispatch2::DispatchQueueAttr::SERIAL,
        );
        unsafe {
            stream.addStreamOutput_type_sampleHandlerQueue_error(
                ProtocolObject::from_ref(&*output),
                SCStreamOutputType::Audio,
                Some(&queue),
            )
        }
        .map_err(|error| {
            anyhow::anyhow!(
                "PC音声の出力を登録できません: {}",
                error.localizedDescription()
            )
        })?;
        let (done_tx, done_rx) = sync_channel::<Option<String>>(1);
        let started = RcBlock::new(move |error: *mut NSError| {
            let _ = done_tx.try_send(error_text(error));
        });
        unsafe { stream.startCaptureWithCompletionHandler(Some(&started)) };
        if let Some(error) = wait(done_rx, "PC音声の取得開始")? {
            bail!("{PERMISSION_HELP}（{error}）");
        }
        Ok(Self {
            stream,
            _output: output,
            packets,
        })
    }
}

impl Drop for SystemAudio {
    fn drop(&mut self) {
        let (tx, rx) = sync_channel::<()>(1);
        let stopped = RcBlock::new(move |_error: *mut NSError| {
            let _ = tx.try_send(());
        });
        unsafe { self.stream.stopCaptureWithCompletionHandler(Some(&stopped)) };
        let _ = rx.recv_timeout(Duration::from_secs(3));
    }
}

/// Text in the image, one recognized line per row, top to bottom.
pub fn ocr(image: &image::RgbaImage, language: &str) -> Result<String> {
    use objc2_core_foundation::CFData;
    use objc2_core_graphics::{
        CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage,
        CGImageAlphaInfo,
    };
    use objc2_vision::{
        VNImageRequestHandler, VNRecognizeTextRequest, VNRequest, VNRequestTextRecognitionLevel,
    };
    ensure!(
        image.width() > 0 && image.height() > 0,
        "OCR image is empty"
    );
    let (width, height) = (image.width() as usize, image.height() as usize);
    let data = CFData::from_bytes(image.as_raw());
    let provider = CGDataProvider::with_cf_data(Some(&data)).context("画像を準備できません。")?;
    let space = CGColorSpace::new_device_rgb().context("画像を準備できません。")?;
    let cg_image = unsafe {
        CGImage::new(
            width,
            height,
            8,
            32,
            width * 4,
            Some(&space),
            CGBitmapInfo(CGImageAlphaInfo::NoneSkipLast.0),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
    .context("画像を準備できません。")?;
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    let languages = vision_languages(language);
    if languages.is_empty() {
        request.setAutomaticallyDetectsLanguage(true);
    } else {
        let tags: Vec<Retained<NSString>> = languages
            .iter()
            .map(|tag| NSString::from_str(tag))
            .collect();
        request.setRecognitionLanguages(&NSArray::from_retained_slice(&tags));
    }
    let handler = unsafe {
        VNImageRequestHandler::initWithCGImage_options(
            VNImageRequestHandler::alloc(),
            &cg_image,
            &objc2_foundation::NSDictionary::new(),
        )
    };
    let base: &VNRequest = &request;
    handler
        .performRequests_error(&NSArray::from_slice(&[base]))
        .map_err(|error| {
            anyhow::anyhow!("文字認識に失敗しました: {}", error.localizedDescription())
        })?;
    let mut lines = Vec::new();
    if let Some(results) = request.results() {
        let mut found: Vec<(f64, f64, String)> = Vec::new();
        for observation in results.iter() {
            let candidates = observation.topCandidates(1);
            let Some(best) = candidates.firstObject() else {
                continue;
            };
            let text = best.string().to_string();
            if text.trim().is_empty() {
                continue;
            }
            let bounds = unsafe { observation.boundingBox() };
            // Vision measures from the bottom-left corner.
            found.push((
                1.0 - (bounds.origin.y + bounds.size.height),
                bounds.origin.x,
                text,
            ));
        }
        found.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.total_cmp(&b.1)));
        lines.extend(found.into_iter().map(|(_, _, text)| text));
    }
    Ok(lines.join("\n"))
}

/// Vision language tags for the OCR language setting. Empty means automatic.
fn vision_languages(requested: &str) -> Vec<&'static str> {
    let requested = requested.trim().replace('_', "-").to_ascii_lowercase();
    match requested.as_str() {
        "" | "auto" => Vec::new(),
        "ja" | "ja-jp" => vec!["ja-JP", "en-US"],
        "en" | "en-us" | "en-gb" => vec!["en-US"],
        "zh-hans" | "zh-cn" | "zh-sg" | "zh" => vec!["zh-Hans", "en-US"],
        "zh-hant" | "zh-tw" | "zh-hk" | "zh-mo" => vec!["zh-Hant", "en-US"],
        "ko" | "ko-kr" => vec!["ko-KR", "en-US"],
        "fr" | "fr-fr" => vec!["fr-FR"],
        "de" | "de-de" => vec!["de-DE"],
        "es" | "es-es" => vec!["es-ES"],
        "it" | "it-it" => vec!["it-IT"],
        "pt" | "pt-br" | "pt-pt" => vec!["pt-BR"],
        "ru" | "ru-ru" => vec!["ru-RU"],
        "uk" | "uk-ua" => vec!["uk-UA"],
        "th" | "th-th" => vec!["th-TH"],
        "vi" | "vi-vn" => vec!["vi-VT"],
        "ar" => vec!["ar-SA"],
        _ => Vec::new(),
    }
}

/// Physical memory, and memory not in active use (free, inactive, purgeable).
pub fn memory() -> (Option<u64>, Option<u64>) {
    let mut total: u64 = 0;
    let mut size = std::mem::size_of::<u64>();
    let name = c"hw.memsize";
    let total = (unsafe {
        libc::sysctlbyname(
            name.as_ptr(),
            (&mut total as *mut u64).cast(),
            &mut size,
            std::ptr::null_mut(),
            0,
        )
    } == 0)
        .then_some(total);
    #[allow(deprecated)]
    let available = unsafe {
        let mut stats: libc::vm_statistics64 = std::mem::zeroed();
        let mut count = libc::HOST_VM_INFO64_COUNT;
        let page = libc::sysconf(libc::_SC_PAGESIZE) as u64;
        (libc::host_statistics64(
            libc::mach_host_self(),
            libc::HOST_VM_INFO64,
            (&mut stats as *mut libc::vm_statistics64).cast(),
            &mut count,
        ) == 0)
            .then(|| {
                (stats.free_count as u64
                    + stats.inactive_count as u64
                    + stats.purgeable_count as u64)
                    * page
            })
    };
    (total, available)
}

/// Free bytes on the volume that holds `path`.
pub fn disk_free(path: &std::path::Path) -> Option<u64> {
    use std::os::unix::ffi::OsStrExt;
    let path = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let mut stats: libc::statvfs = unsafe { std::mem::zeroed() };
    (unsafe { libc::statvfs(path.as_ptr(), &mut stats) } == 0)
        .then(|| stats.f_bavail as u64 * stats.f_frsize as u64)
}

/// The Apple GPU, its name and how much memory Metal recommends using
/// (the share of unified memory the GPU may wire), and how much of it is free.
pub fn metal_gpu() -> Option<(String, u64, u64)> {
    use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice};
    let device = MTLCreateSystemDefaultDevice()?;
    let budget = device.recommendedMaxWorkingSetSize();
    let used = device.currentAllocatedSize() as u64;
    Some((
        device.name().to_string(),
        budget,
        budget.saturating_sub(used),
    ))
}

/// The process listening on 127.0.0.1:`port`, as `lsof` (part of every macOS)
/// reports it. Used like Windows' GetExtendedTcpTable, to make sure a local
/// model server is the child this app started.
pub fn listener_owner(port: u16) -> Result<Option<u32>> {
    let output = std::process::Command::new("/usr/sbin/lsof")
        .args([
            "-nP",
            "-a",
            &format!("-iTCP@127.0.0.1:{port}"),
            "-sTCP:LISTEN",
            "-Fp",
        ])
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .context("ローカル接続の所有者を確認できませんでした。")?;
    // `lsof` exits 1 when nothing matches.
    let pids: std::collections::BTreeSet<u32> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.strip_prefix('p')?.parse().ok())
        .collect();
    ensure!(
        pids.len() <= 1,
        "ローカル接続を複数のプロセスが待ち受けています。"
    );
    Ok(pids.into_iter().next())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converter_passes_16k_through_and_halves_32k() {
        let mut c = Converter::default();
        assert_eq!(c.convert(&[0.5, 2.0], 16_000.0), vec![0.5, 1.0]);
        let input: Vec<f32> = (0..320).map(|i| i as f32 / 1000.0).collect();
        let mut c = Converter::default();
        let a = c.convert(&input[..160], 32_000.0);
        let b = c.convert(&input[160..], 32_000.0);
        assert_eq!(a.len() + b.len(), 160);
    }
}
