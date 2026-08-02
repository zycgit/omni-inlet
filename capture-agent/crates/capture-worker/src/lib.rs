pub mod ffmpeg;
pub mod platform;
pub mod registry;
pub mod runner;
pub mod source;
pub mod test_pattern;
#[cfg(target_os = "linux")]
pub mod x11;

#[path = "entry/capture_agent.rs"]
pub mod capture_agent_entry;
#[path = "entry/window_enumerator.rs"]
pub mod window_enumerator_entry;
