use crate::wasm_processor::ProcessorHost;
use firewheel::{
    StreamInfo,
    backend::{AudioBackend, DeviceInfo},
    collector::ArcGc,
    processor::FirewheelProcessor,
};
use std::{
    cell::RefCell,
    num::NonZeroU32,
    rc::Rc,
    sync::{atomic::AtomicBool, mpsc},
};
use web_sys::{AudioContext, AudioContextOptions, AudioWorkletNode};

/// The main-thread host for the Web Audio API backend.
///
/// This backend relies on Wasm multi-threading. The Firewheel
/// audio nodes are processed within a Web Audio `AudioWorkletNode`
/// that shares its memory with the initializing Wasm module.
///
/// When dropped, the underlying `AudioContext` is closed and all
/// resources are released.
pub struct WebAudioBackend {
    processor: mpsc::Sender<FirewheelProcessor>,
    is_dropped: bool,
    alive: ArcGc<AtomicBool>,
    web_context: AudioContext,
    processor_node: Rc<RefCell<Option<AudioWorkletNode>>>,
}

impl Drop for WebAudioBackend {
    fn drop(&mut self) {
        self.alive
            .store(false, std::sync::atomic::Ordering::Relaxed);

        if let Some(node) = self.processor_node.borrow().as_ref() {
            if let Err(e) = node.disconnect() {
                log::error!("Failed to disconnect `AudioWorkletNode`: {e:?}");
            }
        }

        if let Err(e) = self.web_context.close() {
            log::error!("Failed to close `AudioContext`: {e:?}");
        }
    }
}

impl core::fmt::Debug for WebAudioBackend {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WasmBackend")
            .field("is_dropped", &self.is_dropped)
            .field("alive", &self.alive)
            .field("web_context", &self.web_context)
            .finish_non_exhaustive()
    }
}

/// Errors related to initializing the web audio stream.
#[derive(Debug)]
pub enum WebAudioStartError {
    /// An error occurred during Web Audio context initialization.
    Initialization(String),
    /// An error occurred when constructing the `AudioWorkletNode`.
    WorkletCreation(String),
}

impl core::fmt::Display for WebAudioStartError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Initialization(e) => {
                write!(f, "Failed to initialize Web Audio API object: {e}")
            }
            Self::WorkletCreation(e) => {
                write!(f, "Failed to create the backend audio worklet: {e}")
            }
        }
    }
}

impl std::error::Error for WebAudioStartError {}

/// Errors encountered while the web audio stream is running.
#[derive(Debug)]
pub enum WebAudioStreamError {
    /// The `AudioWorkletNode` was unexpectedly dropped.
    UnexpectedDrop,
}

impl core::fmt::Display for WebAudioStreamError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedDrop => {
                write!(f, "The `AudioWorkletNode` was unexpectedly dropped")
            }
        }
    }
}

impl std::error::Error for WebAudioStreamError {}

/// The Web Audio backend's configuration.
#[derive(Debug, Default, Clone)]
pub struct WebAudioConfig {
    /// The desired sample rate.
    pub sample_rate: Option<NonZeroU32>,
}

impl AudioBackend for WebAudioBackend {
    type Config = WebAudioConfig;
    type StartStreamError = WebAudioStartError;
    type StreamError = WebAudioStreamError;

    fn available_input_devices() -> Vec<DeviceInfo> {
        vec![]
    }

    fn available_output_devices() -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            name: "default output".into(),
            num_channels: 2,
            is_default: true,
        }]
    }

    fn start_stream(config: Self::Config) -> Result<(Self, StreamInfo), Self::StartStreamError> {
        let (sender, receiver) = mpsc::channel();

        let context = match config.sample_rate {
            Some(sample_rate) => {
                let options = AudioContextOptions::new();
                options.set_sample_rate(sample_rate.get() as f32);
                web_sys::AudioContext::new_with_context_options(&options)
                    .map_err(|e| WebAudioStartError::Initialization(format!("{e:?}")))?
            }
            None => web_sys::AudioContext::new()
                .map_err(|e| WebAudioStartError::Initialization(format!("{e:?}")))?,
        };

        let sample_rate = context.sample_rate();
        let inputs = 0;
        let outputs = 2;

        fn create_buffer(len: usize) -> &'static mut [f32] {
            let mut vec = Vec::new();
            vec.reserve_exact(len);
            vec.extend(std::iter::repeat_n(0f32, len));
            Vec::leak(vec)
        }

        let alive = ArcGc::new(AtomicBool::new(true));
        let wrapper = ProcessorHost {
            processor: None,
            receiver,
            alive: alive.clone(),
            inputs,
            input_buffers: create_buffer(inputs * crate::BLOCK_FRAMES),
            outputs,
            output_buffers: create_buffer(outputs * crate::BLOCK_FRAMES),
        };
        let wrapper = wrapper.pack();

        let processor_node = Rc::new(RefCell::new(None));
        let prepare_worklet = {
            let context = context.clone();
            let processor_node = processor_node.clone();
            async move {
                let mod_url = crate::dynamic_module::dependent_module!("./js/audio-worklet.js")?;
                wasm_bindgen_futures::JsFuture::from(
                    context
                        .audio_worklet()?
                        .add_module(mod_url.trim_start_matches('.'))?,
                )
                .await?;

                let node =
                    web_sys::AudioWorkletNode::new_with_options(&context, "WasmProcessor", &{
                        let options = web_sys::AudioWorkletNodeOptions::new();

                        let output_channels = js_sys::Array::new_with_length(1);
                        output_channels.set(0, outputs.into());
                        options.set_output_channel_count(&output_channels);

                        options.set_processor_options(Some(&js_sys::Array::of3(
                            &wasm_bindgen::module(),
                            &wasm_bindgen::memory(),
                            &wrapper.into(),
                        )));
                        options
                    })?;

                node.connect_with_audio_node(&context.destination())?;
                *processor_node.borrow_mut() = Some(node);

                Ok::<_, wasm_bindgen::JsValue>(())
            }
        };

        wasm_bindgen_futures::spawn_local(async move {
            if let Err(e) = prepare_worklet.await {
                log::error!("failed to initialize audio worklet: {e:?}");
            }
        });

        Ok((
            Self {
                web_context: context,
                is_dropped: false,
                processor: sender,
                processor_node,
                alive,
            },
            StreamInfo {
                sample_rate: NonZeroU32::new(sample_rate as u32)
                    .expect("Web Audio API sample rate should be non-zero"),
                max_block_frames: NonZeroU32::new(crate::BLOCK_FRAMES as u32).unwrap(),
                num_stream_in_channels: inputs as u32,
                num_stream_out_channels: outputs as u32,
                input_device_name: None,
                output_device_name: Some("default output".into()),
                ..Default::default()
            },
        ))
    }

    fn set_processor(&mut self, processor: FirewheelProcessor) {
        if self.processor.send(processor).is_err() {
            self.is_dropped = true;
        }
    }

    fn poll_status(&mut self) -> Result<(), Self::StreamError> {
        if self.is_dropped {
            Err(WebAudioStreamError::UnexpectedDrop)
        } else {
            Ok(())
        }
    }
}
