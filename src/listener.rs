//! PulseAudio event subscription management.

use std::cell::RefCell;
use std::rc::Rc;

use libpulse_binding as pulse;
use pulse::context::subscribe::{Facility, InterestMaskSet, Operation};
use pulse::context::Context;

/// Human-readable name for a PulseAudio subscription facility.
fn facility_name(f: Facility) -> &'static str {
    match f {
        Facility::Sink => "sink",
        Facility::Source => "source",
        Facility::SinkInput => "sink-input",
        Facility::SourceOutput => "source-output",
        Facility::Module => "module",
        Facility::Client => "client",
        Facility::SampleCache => "sample-cache",
        Facility::Server => "server",
        Facility::Card => "card",
    }
}

/// Human-readable name for a PulseAudio subscription event type.
fn operation_name(o: Operation) -> &'static str {
    match o {
        Operation::New => "new",
        Operation::Changed => "change",
        Operation::Removed => "remove",
    }
}

/// Manages PulseAudio event subscription and callback dispatch.
///
/// Subscribe to all event types after the context is `Ready`, then prints
/// each event's type, facility and index to stdout.
pub struct PulseEventListener {
    context: Rc<RefCell<Context>>,
}

impl PulseEventListener {
    /// Create a new listener bound to the given PulseAudio context.
    pub fn new(context: Rc<RefCell<Context>>) -> Self {
        Self { context }
    }

    /// Subscribe to all PulseAudio event types and install the event callback.
    pub fn subscribe(&self) {
        // Register interest in every facility.  The returned Operation is
        // dropped immediately — the subscription persists on the server side
        // and events arrive through the callback set below.
        self.context
            .borrow_mut()
            .subscribe(InterestMaskSet::all(), |_success| {});

        // Install the per-event callback.  The closure is purely functional
        // (no captured state), so it trivially satisfies `'static`.
        self.context
            .borrow_mut()
            .set_subscribe_callback(Some(Self::make_callback()));
    }

    /// Factory – produces the boxed event callback.
    fn make_callback() -> Box<dyn FnMut(Option<Facility>, Option<Operation>, u32)> {
        Box::new(|facility, operation, idx| {
            let f = facility.map(facility_name).unwrap_or("unknown");
            let o = operation.map(operation_name).unwrap_or("unknown");
            println!("[{}] {} idx={}", o, f, idx);
        })
    }
}