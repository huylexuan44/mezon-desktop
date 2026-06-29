use gpui::{App, AppContext, Entity, Global};
use std::sync::Arc;

#[derive(Debug, Clone)]
pub struct AudioDeviceInfo {
    pub id: String,
    pub name: String,
}

pub type MicCaptureHandle = Box<dyn Send>;

pub type MicCaptureFactory =
    Arc<dyn Fn(&str, flume::Sender<f32>) -> Result<MicCaptureHandle, String> + Send + Sync>;

pub struct AudioStore {
    pub input_devices: Vec<AudioDeviceInfo>,
    pub output_devices: Vec<AudioDeviceInfo>,
    pub mic_capture_factory: Option<MicCaptureFactory>,
}

impl AudioStore {
    pub fn init(cx: &mut App) -> Entity<Self> {
        let entity = cx.new(|_| Self {
            input_devices: Vec::new(),
            output_devices: Vec::new(),
            mic_capture_factory: None,
        });
        cx.set_global(GlobalAudioStore(entity.clone()));
        entity
    }

    pub fn global(cx: &App) -> Entity<Self> {
        cx.global::<GlobalAudioStore>().0.clone()
    }

    pub fn try_global(cx: &App) -> Option<Entity<Self>> {
        cx.try_global::<GlobalAudioStore>().map(|g| g.0.clone())
    }

    pub fn set_devices(
        entity: &Entity<Self>,
        input: Vec<AudioDeviceInfo>,
        output: Vec<AudioDeviceInfo>,
        cx: &mut App,
    ) {
        entity.update(cx, |store, cx| {
            store.input_devices = input;
            store.output_devices = output;
            cx.notify();
        });
    }

    pub fn set_mic_capture_factory(
        entity: &Entity<Self>,
        factory: MicCaptureFactory,
        cx: &mut App,
    ) {
        entity.update(cx, |store, cx| {
            store.mic_capture_factory = Some(factory);
            cx.notify();
        });
    }

    pub fn start_mic_capture(
        &self,
        device_id: &str,
        sender: flume::Sender<f32>,
    ) -> Result<MicCaptureHandle, String> {
        match &self.mic_capture_factory {
            Some(factory) => factory(device_id, sender),
            None => Err("Mic capture not available on this platform".to_string()),
        }
    }
}

impl gpui::EventEmitter<()> for AudioStore {}

struct GlobalAudioStore(Entity<AudioStore>);
impl Global for GlobalAudioStore {}
