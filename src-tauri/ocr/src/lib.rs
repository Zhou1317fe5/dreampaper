#[cfg(feature = "engine")]
pub mod engine;
pub mod pad;
#[cfg(feature = "engine")]
pub mod ppocr;
pub mod protocol;
pub mod supervisor;

/// Version of the sidecar crate; the host shows it as the engine version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// ONNX Runtime release the sidecar is built and gated against. The dynamic
/// library shipped with the app must come from this tag.
pub const RUNTIME_VERSION: &str = "1.29.0";
/// Substring the sidecar's `ready` frame must report for the pinned runtime
/// (`git-commit-id` of tag v1.29.0).
pub const RUNTIME_COMMIT: &str = "2e2543f";
