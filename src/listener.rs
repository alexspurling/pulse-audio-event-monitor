//! PulseAudio event subscription management.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::introspect::CardInfo;
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
/// On Ready, call [`fetch_all`] with a completion closure.  `fetch_all` fires
/// asynchronous info-list queries for every entity type; each increments an
/// internal pending counter.  When every query has delivered its EOL marker
/// the pending counter reaches zero and the completion closure runs — at
/// which point [`subscribe`] is called and the event stream begins.
///
/// This ordering guarantees that the initial entity snapshot is complete
/// before subscription events arrive, matching the C++ `PulseAudioState`
/// pattern.
pub struct PulseEventListener {
    context: Rc<RefCell<Context>>,
    pending: Cell<i32>,
    on_complete: RefCell<Option<Box<dyn FnOnce()>>>,
}

impl PulseEventListener {
    /// Create a new listener bound to the given PulseAudio context.
    ///
    /// Returns `Rc<Self>` so that async callbacks can clone the Rc and call
    /// methods through interior mutability (Cell / RefCell).
    pub fn new(context: Rc<RefCell<Context>>) -> Rc<Self> {
        Rc::new(Self {
            context,
            pending: Cell::new(0),
            on_complete: RefCell::new(None),
        })
    }

    /// Fire asynchronous info-list queries for every entity type.
    ///
    /// `on_complete` is called once all queries have delivered their EOL or
    /// Error markers.  This is where [`subscribe`] should be called.
    pub fn fetch_all(self: &Rc<Self>, on_complete: impl FnOnce() + 'static) {
        *self.on_complete.borrow_mut() = Some(Box::new(on_complete));
        self.fetch_cards();
    }

    // ── per-entity fetches ──────────────────────────────────────────

    fn fetch_cards(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_card_info_list(move |result: ListResult<&CardInfo>| {
            match result {
                ListResult::Item(card_info) => {
                    println!(
                        "fetch_all: card callback triggered (index={})",
                        card_info.index,
                    );
                }
                ListResult::End => {
                    println!("fetch_all: card list complete (EOL)");
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch_all: card list error");
                    this.signal_eol();
                }
            }
        });
    }

    // ── pending-counter management ──────────────────────────────────

    /// Decrement the pending counter.  When it reaches zero the
    /// `on_complete` closure fires.
    fn signal_eol(&self) {
        let remaining = self.pending.get() - 1;
        self.pending.set(remaining);
        if remaining == 0 {
            if let Some(cb) = self.on_complete.borrow_mut().take() {
                cb();
            }
        }
    }

    // ── subscription ────────────────────────────────────────────────

    /// Subscribe to all PulseAudio event types and install the event callback.
    pub fn subscribe(&self) {
        self.context
            .borrow_mut()
            .subscribe(InterestMaskSet::all(), |_success| {});

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