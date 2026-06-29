use std::sync::OnceLock;
use tokio::runtime::Runtime;

static VOICE_RUNTIME: OnceLock<Runtime> = OnceLock::new();

pub(crate) fn runtime() -> &'static Runtime {
    VOICE_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .worker_threads(2)
            .thread_name("mezon-voice")
            .build()
            .expect("failed to build mezon-voice runtime")
    })
}

pub(crate) fn handle() -> tokio::runtime::Handle {
    runtime().handle().clone()
}
