//! PulseAudio event subscription management.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::introspect::{
    CardInfo, ClientInfo, ModuleInfo, SampleInfo, ServerInfo, SinkInfo, SinkInputInfo,
    SourceInfo, SourceOutputInfo,
};
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

/// Owned snapshot of PulseAudio entities fetched during initialisation.
///
/// Each entity type has a map keyed by its PulseAudio index.  The maps are
/// populated during [`fetch_all`](PulseEventListener::fetch_all) before the
/// subscription callback is installed, so they represent the complete initial
/// state.
pub struct PulseAudioState {
    pub cards: HashMap<u32, CardInfo<'static>>,
    pub sinks: HashMap<u32, SinkInfo<'static>>,
    pub sources: HashMap<u32, SourceInfo<'static>>,
    pub sink_inputs: HashMap<u32, SinkInputInfo<'static>>,
    pub source_outputs: HashMap<u32, SourceOutputInfo<'static>>,
    pub modules: HashMap<u32, ModuleInfo<'static>>,
    pub clients: HashMap<u32, ClientInfo<'static>>,
    pub samples: HashMap<u32, SampleInfo<'static>>,
    pub server: Option<ServerInfo<'static>>,
}

impl PulseAudioState {
    fn new() -> Self {
        Self {
            cards: HashMap::new(),
            sinks: HashMap::new(),
            sources: HashMap::new(),
            sink_inputs: HashMap::new(),
            source_outputs: HashMap::new(),
            modules: HashMap::new(),
            clients: HashMap::new(),
            samples: HashMap::new(),
            server: None,
        }
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
    state: RefCell<PulseAudioState>,
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
            state: RefCell::new(PulseAudioState::new()),
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
        self.fetch_sinks();
        self.fetch_sources();
        self.fetch_sink_inputs();
        self.fetch_source_outputs();
        self.fetch_modules();
        self.fetch_clients();
        self.fetch_samples();
        self.fetch_server();
    }

    // ── per-entity fetches (list-style) ──────────────────────────────

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
                    let owned = card_info.to_owned();
                    this.state.borrow_mut().cards.insert(owned.index, owned);
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

    fn fetch_sinks(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_sink_info_list(move |result: ListResult<&SinkInfo>| {
            match result {
                ListResult::Item(info) => {
                    println!("fetch_all: sink callback triggered (index={})", info.index);
                    let owned = info.to_owned();
                    this.state.borrow_mut().sinks.insert(owned.index, owned);
                }
                ListResult::End => {
                    println!("fetch_all: sink list complete (EOL)");
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch_all: sink list error");
                    this.signal_eol();
                }
            }
        });
    }

    fn fetch_sources(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_source_info_list(move |result: ListResult<&SourceInfo>| {
            match result {
                ListResult::Item(info) => {
                    println!("fetch_all: source callback triggered (index={})", info.index);
                    let owned = info.to_owned();
                    this.state.borrow_mut().sources.insert(owned.index, owned);
                }
                ListResult::End => {
                    println!("fetch_all: source list complete (EOL)");
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch_all: source list error");
                    this.signal_eol();
                }
            }
        });
    }

    fn fetch_sink_inputs(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op =
            introspector.get_sink_input_info_list(move |result: ListResult<&SinkInputInfo>| {
                match result {
                    ListResult::Item(info) => {
                        println!(
                            "fetch_all: sink-input callback triggered (index={})",
                            info.index,
                        );
                        let owned = info.to_owned();
                        this.state.borrow_mut().sink_inputs.insert(owned.index, owned);
                    }
                    ListResult::End => {
                        println!("fetch_all: sink-input list complete (EOL)");
                        this.signal_eol();
                    }
                    ListResult::Error => {
                        eprintln!("fetch_all: sink-input list error");
                        this.signal_eol();
                    }
                }
            });
    }

    fn fetch_source_outputs(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_source_output_info_list(
            move |result: ListResult<&SourceOutputInfo>| {
                match result {
                    ListResult::Item(info) => {
                        println!(
                            "fetch_all: source-output callback triggered (index={})",
                            info.index,
                        );
                        let owned = info.to_owned();
                        this.state.borrow_mut().source_outputs.insert(owned.index, owned);
                    }
                    ListResult::End => {
                        println!("fetch_all: source-output list complete (EOL)");
                        this.signal_eol();
                    }
                    ListResult::Error => {
                        eprintln!("fetch_all: source-output list error");
                        this.signal_eol();
                    }
                }
            },
        );
    }

    fn fetch_modules(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_module_info_list(move |result: ListResult<&ModuleInfo>| {
            match result {
                ListResult::Item(info) => {
                    println!(
                        "fetch_all: module callback triggered (index={})",
                        info.index,
                    );
                    let owned = info.to_owned();
                    this.state.borrow_mut().modules.insert(owned.index, owned);
                }
                ListResult::End => {
                    println!("fetch_all: module list complete (EOL)");
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch_all: module list error");
                    this.signal_eol();
                }
            }
        });
    }

    fn fetch_clients(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_client_info_list(move |result: ListResult<&ClientInfo>| {
            match result {
                ListResult::Item(info) => {
                    println!(
                        "fetch_all: client callback triggered (index={})",
                        info.index,
                    );
                    let owned = info.to_owned();
                    this.state.borrow_mut().clients.insert(owned.index, owned);
                }
                ListResult::End => {
                    println!("fetch_all: client list complete (EOL)");
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch_all: client list error");
                    this.signal_eol();
                }
            }
        });
    }

    fn fetch_samples(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_sample_info_list(move |result: ListResult<&SampleInfo>| {
            match result {
                ListResult::Item(info) => {
                    println!(
                        "fetch_all: sample callback triggered (index={})",
                        info.index,
                    );
                    let owned = info.to_owned();
                    this.state.borrow_mut().samples.insert(owned.index, owned);
                }
                ListResult::End => {
                    println!("fetch_all: sample list complete (EOL)");
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch_all: sample list error");
                    this.signal_eol();
                }
            }
        });
    }

    // ── per-entity fetches (single-value) ───────────────────────────

    fn fetch_server(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_server_info(move |server_info: &ServerInfo| {
            println!("fetch_all: server callback triggered");
            let owned = server_info.to_owned();
            this.state.borrow_mut().server = Some(owned);
            this.signal_eol();
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