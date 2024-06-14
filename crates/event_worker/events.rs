use base_mem_check::MemCheckState;
use serde::{Deserialize, Serialize};
use strum::IntoStaticStr;
use uuid::Uuid;

#[derive(Serialize, Deserialize, Debug)]
pub struct BootEvent {
    pub boot_time: usize,
}
#[derive(Serialize, Deserialize, Debug)]
pub struct BootFailureEvent {
    pub msg: String,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerMemoryUsed {
    pub total: usize,
    pub heap: usize,
    pub external: usize,
    pub mem_check_captured: MemCheckState,
}

#[derive(Serialize, Deserialize, IntoStaticStr, Debug, Clone, Copy)]
#[serde(tag = "limited_by")]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
pub enum MemoryLimitDetail {
    MemCheck,
    V8,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum ShutdownReason {
    WallClockTime,
    CPUTime,
    Memory(MemoryLimitDetail),
    EarlyDrop,
    TerminationRequested,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct ShutdownEvent {
    pub reason: ShutdownReason,
    pub cpu_time_used: usize,
    pub memory_used: WorkerMemoryUsed,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub struct UncaughtExceptionEvent {
    pub exception: String,
    pub cpu_time_used: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct EventLoopCompletedEvent {
    pub cpu_time_used: usize,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct LogEvent {
    pub msg: String,
    pub level: LogLevel,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum LogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Serialize, Deserialize, Debug)]
pub enum WorkerEvents {
    Boot(BootEvent),
    BootFailure(BootFailureEvent),
    UncaughtException(UncaughtExceptionEvent),
    Shutdown(ShutdownEvent),
    EventLoopCompleted(EventLoopCompletedEvent),
    Log(LogEvent),
}

impl WorkerEvents {
    pub fn with_cpu_time_used(mut self, cpu_time_used_ms: usize) -> Self {
        match &mut self {
            Self::UncaughtException(UncaughtExceptionEvent { cpu_time_used, .. })
            | Self::Shutdown(ShutdownEvent { cpu_time_used, .. }) => {
                *cpu_time_used = cpu_time_used_ms;
            }

            _ => {}
        }

        self
    }
}

#[derive(Serialize, Deserialize, Default, Debug, Clone)]
pub struct EventMetadata {
    pub service_path: Option<String>,
    pub execution_id: Option<Uuid>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkerEventWithMetadata {
    pub event: WorkerEvents,
    pub metadata: EventMetadata,
}

#[derive(Serialize, Deserialize)]
pub enum RawEvent {
    Event(Box<WorkerEventWithMetadata>),
    Done,
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct IncomingEvent {
    event_type: Option<String>,
    data: Option<Vec<u8>>,
    done: bool,
}
