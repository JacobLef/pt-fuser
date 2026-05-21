pub struct DataCollectionError;

impl DataCollectionError {
    pub const ID: u32 = 1;
    pub const NAME: &'static str = "TraceError";
    pub const DESC: &'static str = "Trace decoder hit an error. Callstacks may be corrupted.";
}

pub struct TraceInterrupted;

impl TraceInterrupted {
    pub const ID: u32 = 2710682459;
    pub const NAME: &'static str = "Interrupt";
    pub const DESC: &'static str = "Trace was paused while the CPU serviced an interrupt.";
}

pub struct LostFrameWhileMerging;

impl LostFrameWhileMerging {
    pub const ID: u32 = 555740177;
    pub const NAME: &'static str = "Lost Frames";
    pub const DESC: &'static str =
        "A frame could not be added while merging because it overlapped with adjacent frames.";
}
