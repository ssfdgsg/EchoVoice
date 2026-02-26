use crate::audio::mixer::Mixer;
use ringbuf::traits::Observer;
use ringbuf::traits::{Consumer, Producer, Split};
use ringbuf::HeapRb;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, Ordering},
    Arc,
};
use std::thread;
use tracing::{error, info};
use wasapi::{AudioCaptureClient, DeviceCollection, DeviceEnumerator, Direction, StreamMode};

struct SafeAudioCaptureClient(AudioCaptureClient);
unsafe impl Send for SafeAudioCaptureClient {}

impl std::ops::Deref for SafeAudioCaptureClient {
    type Target = AudioCaptureClient;
    fn deref(&self) -> &AudioCaptureClient {
        &self.0
    }
}

pub struct AudioEngine {
    is_running: Arc<AtomicBool>,
    mixer: Arc<Mixer>,
    sample_rate: Arc<AtomicU32>,
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    pub fn new() -> Self {
        Self {
            is_running: Arc::new(AtomicBool::new(false)),
            mixer: Arc::new(Mixer::new()),
            sample_rate: Arc::new(AtomicU32::new(48000)),
        }
    }

    pub fn get_mixer(&self) -> Arc<Mixer> {
        self.mixer.clone()
    }

    pub fn start_bridge(
        &self,
        input_device_id: String,
        output_device_id: String,
    ) -> Result<(), String> {
        if self.is_running.load(Ordering::SeqCst) {
            return Err("Engine is already running".to_string());
        }

        self.is_running.store(true, Ordering::SeqCst);
        let is_running_clone = self.is_running.clone();

        info!(
            "Starting audio bridge from {} to {}",
            input_device_id, output_device_id
        );

        let mixer_clone = self.mixer.clone();
        let sr_clone = self.sample_rate.clone();
        thread::spawn(move || {
            if let Err(e) = run_bridge_loop(
                input_device_id,
                output_device_id,
                is_running_clone,
                mixer_clone,
                sr_clone,
            ) {
                error!("Bridge loop error: {}", e);
            }
        });

        Ok(())
    }

    pub fn stop_bridge(&self) {
        info!("Stopping audio bridge");
        self.is_running.store(false, Ordering::SeqCst);
        self.mixer.clear();
    }

    pub fn play_sound(&self, file_path: &str) -> Result<(), String> {
        if !self.is_running.load(Ordering::Relaxed) {
            return Err("Bridge is not running. Cannot play sound.".to_string());
        }

        let sr = self.sample_rate.load(Ordering::Relaxed);
        let bank = crate::audio::fx::SoundBank::load_from_file(file_path, sr)?;
        info!(
            "Loaded sound: {} ({} samples, {} sr)",
            file_path,
            bank.data.len(),
            sr
        );
        self.mixer.play_sound(Arc::new(bank.data), 1.0);
        Ok(())
    }
}

fn get_device_by_id(dir: Direction, target_id: &str) -> Result<wasapi::Device, String> {
    let enumerator =
        DeviceEnumerator::new().map_err(|e| format!("Failed to get enumerator: {}", e))?;

    let collection = enumerator
        .get_device_collection(&dir)
        .map_err(|e| format!("Failed to get device collection: {}", e))?;

    let count = collection
        .get_nbr_devices()
        .map_err(|e| format!("Failed to get device count: {}", e))?;

    for i in 0..count {
        if let Ok(dev) = collection.get_device_at_index(i) {
            if let Ok(id) = dev.get_id() {
                if id == target_id {
                    return Ok(dev);
                }
            }
        }
    }

    Err(format!("Device not found: {}", target_id))
}

fn run_bridge_loop(
    input_device_id: String,
    output_device_id: String,
    is_running: Arc<AtomicBool>,
    mixer: Arc<Mixer>,
    sample_rate_state: Arc<AtomicU32>,
) -> Result<(), String> {
    wasapi::initialize_mta().ok();

    info!("Locating input device: {}", input_device_id);
    let input_device = get_device_by_id(Direction::Capture, &input_device_id)?;
    info!("Locating output device: {}", output_device_id);
    let output_device = get_device_by_id(Direction::Render, &output_device_id)?;

    info!(
        "Activating Input device: {:?}",
        input_device.get_friendlyname()
    );
    info!(
        "Activating Output device: {:?}",
        output_device.get_friendlyname()
    );

    let mut in_client = input_device
        .get_iaudioclient()
        .map_err(|e| format!("In client err: {}", e))?;
    let in_format = in_client
        .get_mixformat()
        .map_err(|e| format!("In format err: {}", e))?;

    let mut out_client = output_device
        .get_iaudioclient()
        .map_err(|e| format!("Out client err: {}", e))?;
    let out_format = out_client
        .get_mixformat()
        .map_err(|e| format!("Out format err: {}", e))?;

    let sr = out_format.get_samplespersec();
    sample_rate_state.store(sr, Ordering::Relaxed);

    info!("Input format: {:?}", in_format.to_waveformatex());
    info!("Output format: {:?}", out_format.to_waveformatex());

    in_client
        .initialize_client(
            &in_format,
            &Direction::Capture,
            &StreamMode::PollingShared {
                autoconvert: false,
                buffer_duration_hns: 0,
            },
        )
        .map_err(|e| format!("In client init err: {}", e))?;

    out_client
        .initialize_client(
            &out_format,
            &Direction::Render,
            &StreamMode::PollingShared {
                autoconvert: false,
                buffer_duration_hns: 0,
            },
        )
        .map_err(|e| format!("Out client init err: {}", e))?;

    let in_capture = SafeAudioCaptureClient(
        in_client
            .get_audiocaptureclient()
            .map_err(|e| format!("In capture err: {}", e))?,
    );
    let out_render = out_client
        .get_audiorenderclient()
        .map_err(|e| format!("Out render err: {}", e))?;

    // f32 buffer: 48000 frames * 2 channels (assuming stereo) * 2 seconds. We just buffer f32 samples directly.
    let channels = in_format.get_nchannels() as usize;
    let buf_size = 48000 * channels * 4;
    let rb = HeapRb::<f32>::new(buf_size);
    let (mut prod, mut cons) = rb.split();

    in_client
        .start_stream()
        .map_err(|e| format!("In start err: {}", e))?;
    out_client
        .start_stream()
        .map_err(|e| format!("Out start err: {}", e))?;

    let is_running_clone = is_running.clone();
    let in_bytes_per_frame = in_format.get_blockalign() as usize;

    let capture_thread = std::thread::spawn(move || {
        wasapi::initialize_mta().ok();
        while is_running_clone.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(5));
            if let Ok(Some(frames)) = in_capture.get_next_packet_size() {
                if frames > 0 {
                    let mut data = vec![0u8; frames as usize * in_bytes_per_frame];
                    if let Ok((_read_frames, _buffer_info)) = in_capture.read_from_device(&mut data)
                    {
                        // Cast the raw bytes to f32 samples
                        let f32_samples: &[f32] = bytemuck::cast_slice(&data);
                        for &sample in f32_samples {
                            let _ = prod.try_push(sample);
                        }
                    }
                }
            }
        }
    });

    let out_block_align = out_format.get_blockalign() as usize;
    let out_channels = out_format.get_nchannels() as usize;

    while is_running.load(Ordering::Relaxed) {
        std::thread::sleep(std::time::Duration::from_millis(5));

        let frames_available = match out_client.get_available_space_in_frames() {
            Ok(f) => f,
            Err(_) => continue,
        };

        if frames_available > 0 {
            let samples_needed = (frames_available as usize) * out_channels;

            if cons.occupied_len() >= samples_needed {
                let mut mic_chunk = vec![0f32; samples_needed];
                for i in 0..samples_needed {
                    if let Some(s) = cons.try_pop() {
                        mic_chunk[i] = s;
                    }
                }

                let mut out_chunk = vec![0f32; samples_needed];
                mixer.process_frames(&mic_chunk, &mut out_chunk);

                let byte_chunk: &[u8] = bytemuck::cast_slice(&out_chunk);
                let _ = out_render.write_to_device(frames_available as usize, byte_chunk, None);
            }
        }
    }

    let _ = capture_thread.join();
    let _ = in_client.stop_stream();
    let _ = out_client.stop_stream();

    info!("Audio bridge loop stopped gracefully");
    Ok(())
}
