//! PulseAudio event subscription management.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;

use libpulse_binding::def::SinkState;
use libpulse_binding::format::Info;
use libpulse_binding::volume::ChannelVolumes;

use libpulse_binding as pulse;
use pulse::callbacks::ListResult;
use pulse::context::introspect::{
    CardInfo, CardPortInfo, CardProfileInfo, ClientInfo, ModuleInfo, SampleInfo, ServerInfo,
    SinkInfo, SinkInputInfo, SinkPortInfo, SourceInfo, SourceOutputInfo, SourcePortInfo,
};
use pulse::context::subscribe::{Facility, InterestMaskSet, Operation};
use pulse::context::Context;

use serde_json::{json, Map, Value};

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

    /// Print the complete state as a JSON object to stdout.
    pub fn print_full_state(&self) {
        let mut root = Map::new();

        root.insert("modules".into(), map_to_json(&self.modules, module_to_json));
        root.insert("sinks".into(), map_to_json(&self.sinks, sink_to_json));
        root.insert("sources".into(), map_to_json(&self.sources, source_to_json));
        root.insert("sink_inputs".into(), map_to_json(&self.sink_inputs, sink_input_to_json));
        root.insert("source_outputs".into(), map_to_json(&self.source_outputs, source_output_to_json));
        root.insert("clients".into(), map_to_json(&self.clients, client_to_json));
        root.insert("samples".into(), map_to_json(&self.samples, sample_to_json));
        root.insert("cards".into(), map_to_json(&self.cards, card_to_json));
        root.insert("server".into(), server_to_json(self.server.as_ref()));

        let output = serde_json::to_string_pretty(&Value::Object(root))
            .unwrap_or_else(|_| "{}".to_string());
        println!("{}", output);
    }

    /// Print only the data for a single facility item at the given index as a JSON object.
    pub fn print_facility(&self, facility: Facility, idx: u32) {
        let result = match facility {
            Facility::Sink => self.sinks.get(&idx).map(|v| sink_to_json(v)),
            Facility::Source => self.sources.get(&idx).map(|v| source_to_json(v)),
            Facility::SinkInput => self.sink_inputs.get(&idx).map(|v| sink_input_to_json(v)),
            Facility::SourceOutput => self.source_outputs.get(&idx).map(|v| source_output_to_json(v)),
            Facility::Module => self.modules.get(&idx).map(|v| module_to_json(v)),
            Facility::Client => self.clients.get(&idx).map(|v| client_to_json(v)),
            Facility::SampleCache => self.samples.get(&idx).map(|v| sample_to_json(v)),
            Facility::Card => self.cards.get(&idx).map(|v| card_to_json(v)),
            _ => None,
        };
        let facility_name = facility_name(facility);
        let Some(item_json) = result else { return };
        let mut root = serde_json::Map::new();
        root.insert("index".into(), Value::from(idx));
        root.insert("event".into(), Value::from("new"));
        root.insert("facility".into(), Value::String(facility_name.to_owned()));
        root.insert("data".into(), item_json);
        let output = serde_json::to_string_pretty(&Value::Object(root))
            .unwrap_or_else(|_| "{}".to_string());
        println!("{}", output);
    }
}

// ── JSON conversion helpers ──────────────────────────────────────────

/// Convert a Proplist into a JSON object with string values.
fn proplist_to_json(pl: &pulse::proplist::Proplist) -> Value {
    let mut map = Map::new();
    for key in pl.iter() {
        if let Some(val) = pl.get_str(&key) {
            map.insert(key, Value::String(val));
        }
    }
    Value::Object(map)
}

/// Convert a channelmap::Position variant name from CamelCase to hyphen-separated lowercase.
///
/// `FrontLeftOfCenter` → `"front-left-of-center"`, `Lfe` → `"lfe"`.
fn channel_position_to_str(pos: &pulse::channelmap::Position) -> String {
    let name = format!("{:?}", pos);
    let mut out = String::with_capacity(name.len());
    for (i, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && i > 0 {
            out.push('-');
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch.to_ascii_lowercase());
        }
    }
    out
}

/// Convert a PulseAudio channel map into a JSON comma-separated string of hyphen-separated position names.
fn channel_map_to_json(cm: &pulse::channelmap::Map) -> Value {
    Value::String(cm.get().iter().map(channel_position_to_str).collect::<Vec<_>>().join(","))
}

/// Convert a HashMap of index→entity into a JSON array of entity objects (sorted by index).
fn map_to_json<T>(map: &HashMap<u32, T>, to_val: fn(&T) -> Value) -> Value {
    let mut items: Vec<Value> = map.iter().map(|(_idx, entity)| to_val(entity)).collect();
    items.sort_by(|a, b| {
        let ia = a.get("index").and_then(Value::as_u64).unwrap_or(0);
        let ib = b.get("index").and_then(Value::as_u64).unwrap_or(0);
        ia.cmp(&ib)
    });
    Value::Array(items)
}

/// Convert channel volumes paired with a channel map into a JSON object keyed by position name.
fn volume_to_json(volumes: &ChannelVolumes, map: &pulse::channelmap::Map) -> Value {
    let pos_names: Vec<String> = map.get().iter().map(|p| channel_position_to_str(p)).collect();
    let vol_slice: &[pulse::volume::Volume] = std::borrow::Borrow::borrow(volumes);
    let mut obj = Map::new();
    for (i, vol) in vol_slice.iter().enumerate() {
        let key = pos_names.get(i).map_or_else(|| format!("channel-{}", i), |s| s.clone());
        let mut entry = Map::new();
        entry.insert("value".into(), Value::from(vol.0));
        entry.insert("value_percent".into(), Value::String(vol.print().trim().to_owned()));
        entry.insert("db".into(), Value::String(vol.print_db()));
        obj.insert(key, Value::Object(entry));
    }
    Value::Object(obj)
}

fn format_to_json(format: &Info) -> Value {
    Value::String({
        let enc = format!("{:?}", format.get_encoding()).to_lowercase();
        let props = format.get_properties().to_string_sep("  ").unwrap_or_default();
        format!("{}, {}", enc, props)
    })
}

// ── CardInfo ────────────────────────────────────────────────────────

// ── CardInfo ────────────────────────────────────────────────────────

fn card_profile_info_to_json(p: &CardProfileInfo) -> Value {
    json!({
        "description": p.description,
        "sinks": p.n_sinks,
        "sources": p.n_sources,
        "priority": p.priority,
        "available": p.available,
    })
}

fn sink_port_info_to_json(p: &SinkPortInfo) -> Value {
    json!({
        "name": p.name,
        "description": p.description,
        "type": format!("{:?}", p.r#type),
        "priority": p.priority,
        "availability_group": p.availability_group.as_deref().unwrap_or(""),
        "availability": match p.available {
            pulse::def::PortAvailable::Yes => "available",
            pulse::def::PortAvailable::No => "unavailable",
            _ => "unknown",
        },
    })
}

fn card_port_info_to_json(p: &CardPortInfo) -> Value {
    json!({
        "description": p.description,
        "type": format!("{:?}", p.r#type),
        "priority": p.priority,
        "latency_offset": format!("{} usec", p.latency_offset),
        "availability_group": p.availability_group.as_deref().unwrap_or(""),
        "availability": match p.available {
            pulse::def::PortAvailable::Yes => "available",
            pulse::def::PortAvailable::No => "not available",
            _ => "unknown",
        },
        "properties": proplist_to_json(&p.proplist),
        "profiles": p.profiles.iter().map(|pr| pr.name.clone().map(|n| Value::String(n.into_owned())).unwrap_or(Value::Null)).collect::<Vec<_>>(),
    })
}

fn card_to_json(card: &CardInfo) -> Value {
    json!({
        "index": card.index,
        "name": card.name,
        "driver": card.driver,
        "owner_module": card.owner_module,
        "properties": proplist_to_json(&card.proplist),
        "profiles": card.profiles.iter().map(|p| (p.name.clone().unwrap_or_default().into_owned(), card_profile_info_to_json(p))).collect::<serde_json::Map<_, _>>(),
        "active_profile": card.active_profile.as_ref().and_then(|ap| ap.name.as_deref()).map(|s| Value::String(s.to_owned())),
        "ports": card.ports.iter().map(|p| (p.name.clone().unwrap_or_default().into_owned(), card_port_info_to_json(p))).collect::<serde_json::Map<_, _>>(),
    })
}

/// Convert a SinkState to a lowercase JSON string.
fn sink_state_to_json(state: &SinkState) -> Value {
    Value::String(match state {
        SinkState::Invalid => "INVALID",
        SinkState::Running => "RUNNING",
        SinkState::Idle => "IDLE",
        SinkState::Suspended => "SUSPENDED",
    }.to_owned())
}

fn sink_to_json(sink: &SinkInfo) -> Value {
    json!({
        "index": sink.index,
        "state": sink_state_to_json(&sink.state),
        "name": sink.name,
        "description": sink.description,
        "driver": sink.driver,
        "sample_specification": sink.sample_spec.print(),
        "channel_map": channel_map_to_json(&sink.channel_map),
        "owner_module": sink.owner_module,
        "mute": sink.mute,
        "volume": volume_to_json(&sink.volume, &sink.channel_map),
        "balance": (sink.volume.get_balance(&sink.channel_map) as f64 * 100.0).round() / 100.0,
        "base_volume": {
            "value": sink.base_volume.0,
            "value_percent": Value::String(sink.base_volume.print().trim().to_owned()),
            "db": Value::String(sink.base_volume.print_db()),
        },
        "monitor_source": sink.monitor_source_name,
        "latency": {
            "actual": sink.latency.0 as f64,
            "configured": sink.configured_latency.0 as f64,
        },
        "flags": sink.flags.iter_names().map(|(name, _)| Value::String(name.to_owned())).collect::<Vec<_>>(),
        "properties": proplist_to_json(&sink.proplist),
        "ports": sink.ports.iter().map(sink_port_info_to_json).collect::<Vec<_>>(),
        "active_port": sink.active_port.as_deref().and_then(|p| p.name.as_deref()).map(|s| Value::String(s.to_owned())),
        "formats": sink.formats.iter().map(|f| Value::String(format!("{:?}", f.get_encoding()).to_lowercase())).collect::<Vec<_>>(),
        "n_volume_steps": sink.n_volume_steps,
        "card": sink.card,
    })
}

fn source_state_to_json(state: &pulse::def::SourceState) -> Value {
    Value::String(match state {
        pulse::def::SourceState::Invalid => "INVALID",
        pulse::def::SourceState::Running => "RUNNING",
        pulse::def::SourceState::Idle => "IDLE",
        pulse::def::SourceState::Suspended => "SUSPENDED",
    }.to_owned())
}

// ── SourceInfo ──────────────────────────────────────────────────────

fn source_port_info_to_json(p: &SourcePortInfo) -> Value {
    json!({
        "name": p.name,
        "description": p.description,
        "type": format!("{:?}", p.r#type),
        "priority": p.priority,
        "availability_group": p.availability_group.as_deref().unwrap_or(""),
        "availability": match p.available {
            pulse::def::PortAvailable::Yes => "available",
            pulse::def::PortAvailable::No => "unavailable",
            _ => "unknown",
        },
    })
}

fn source_to_json(source: &SourceInfo) -> Value {
    json!({
        "index": source.index,
        "state": source_state_to_json(&source.state),
        "name": source.name,
        "description": source.description,
        "driver": source.driver,
        "sample_specification": &source.sample_spec.print(),
        "channel_map": channel_map_to_json(&source.channel_map),
        "owner_module": source.owner_module,
        "mute": source.mute,
        "volume": volume_to_json(&source.volume, &source.channel_map),
        "balance": (source.volume.get_balance(&source.channel_map) as f64 * 100.0).round() / 100.0,
        "base_volume": {
            "value": source.base_volume.0,
            "value_percent": Value::String(source.base_volume.print().trim().to_owned()),
            "db": Value::String(source.base_volume.print_db()),
        },
        "monitor_source": source.monitor_of_sink_name,
        "latency": {
            "actual": source.latency.0 as f64,
            "configured": source.configured_latency.0 as f64,
        },
        "flags": source.flags.iter_names().map(|(name, _)| Value::String(name.to_owned())).collect::<Vec<_>>(),
        "properties": proplist_to_json(&source.proplist),
        "ports": source.ports.iter().map(source_port_info_to_json).collect::<Vec<_>>(),
        "active_port": source.active_port.as_deref().and_then(|p| p.name.as_deref()).map(|s| Value::String(s.to_owned())),
        "formats": source.formats.iter().map(|f| Value::String(format!("{:?}", f.get_encoding()).to_lowercase())).collect::<Vec<_>>(),
        "n_volume_steps": source.n_volume_steps,
        "card": source.card,
    })
}

// ── SinkInputInfo ───────────────────────────────────────────────────

fn sink_input_to_json(si: &SinkInputInfo) -> Value {
    json!({
        "index": si.index,
        "driver": si.driver,
        "owner_module": si.owner_module,
        "client": si.client.map(|c| Value::String(c.to_string())).unwrap_or(Value::Null),
        "sink": si.sink,
        "sample_specification": si.sample_spec.print(),
        "channel_map": channel_map_to_json(&si.channel_map),
        "format": format_to_json(&si.format),
        "corked": si.corked,
        "mute": si.mute,
        "volume": volume_to_json(&si.volume, &si.channel_map),
        "balance": (si.volume.get_balance(&si.channel_map) as f64 * 100.0).round() / 100.0,
        "buffer_latency_usec": si.buffer_usec.0 as f64,
        "sink_latency_usec": si.sink_usec.0 as f64,
        "resample_method": si.resample_method.as_deref().map(|s: &str| Value::String(s.to_owned())).unwrap_or(Value::Null),
        "properties": proplist_to_json(&si.proplist),
    })
}

// ── SourceOutputInfo ────────────────────────────────────────────────

fn source_output_to_json(so: &SourceOutputInfo) -> Value {
    json!({
        "index": so.index,
        "name": so.name,
        "owner_module": so.owner_module,
        "client": so.client,
        "source": so.source,
        "sample_specification": so.sample_spec.print(),
        "channel_map": channel_map_to_json(&so.channel_map),
        "buffer_usec": so.buffer_usec.0,
        "source_usec": so.source_usec.0,
        "resample_method": so.resample_method.as_deref().map(|s: &str| Value::String(s.to_owned())).unwrap_or(Value::Null),
        "mute": so.mute,
        "properties": proplist_to_json(&so.proplist),
        "corked": so.corked,
        "has_volume": so.has_volume,
        "volume_writable": so.volume_writable,
        "format": format_to_json(&so.format),
    })
}

fn module_to_json(m: &ModuleInfo) -> Value {
    json!({
        "index": m.index,
        "name": m.name,
        "argument": m.argument.as_deref().unwrap_or(""),
        "usage_counter": m.n_used.map_or(Value::String("n/a".into()), Value::from),
        "properties": proplist_to_json(&m.proplist),
    })
}

fn client_to_json(c: &ClientInfo) -> Value {
    json!({
        "index": c.index,
        "name": c.name,
        "owner_module": c.owner_module,
        "driver": c.driver,
        "properties": proplist_to_json(&c.proplist),
    })
}

// ── SampleInfo ──────────────────────────────────────────────────────

fn sample_to_json(s: &SampleInfo) -> Value {
    json!({
        "index": s.index,
        "name": s.name,
        "volume": volume_to_json(&s.volume, &s.channel_map),
        "sample_specification": s.sample_spec.print(),
        "channel_map": channel_map_to_json(&s.channel_map),
        "duration": s.duration.0,
        "bytes": s.bytes,
        "lazy": s.lazy,
        "filename": s.filename,
    })
}

// ── ServerInfo ──────────────────────────────────────────────────────

fn server_to_json(s: Option<&ServerInfo>) -> Value {
    match s {
        Some(info) => json!({
            "user_name": info.user_name,
            "host_name": info.host_name,
            "server_version": info.server_version,
            "server_name": info.server_name,
            "sample_specification": info.sample_spec.print(),
            "default_sink_name": info.default_sink_name,
            "default_source_name": info.default_source_name,
            "cookie": info.cookie,
            "channel_map": channel_map_to_json(&info.channel_map),
        }),
        None => Value::Null,
    }
}

/// Manages PulseAudio event subscription and callback dispatch.
///
/// On Ready, call [`fetch_all`] with a completion closure.  `fetch_all` fires
/// asynchronous info-list queries for every entity type; each increments an
/// internal pending counter.  When every query has delivered its EOL marker,
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
                    let owned = card_info.to_owned();
                    this.state.borrow_mut().cards.insert(owned.index, owned);
                }
                ListResult::End => {
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch: card list error");
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
                    let owned = info.to_owned();
                    this.state.borrow_mut().sinks.insert(owned.index, owned);
                }
                ListResult::End => {
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch: sink list error");
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
                    let owned = info.to_owned();
                    this.state.borrow_mut().sources.insert(owned.index, owned);
                }
                ListResult::End => {
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch: source list error");
                    this.signal_eol();
                }
            }
        });
    }

    fn fetch_sink_inputs(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_sink_input_info_list(move |result: ListResult<&SinkInputInfo>| {
            match result {
                ListResult::Item(info) => {
                    let owned = info.to_owned();
                    this.state.borrow_mut().sink_inputs.insert(owned.index, owned);
                }
                ListResult::End => {
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch: sink-input list error");
                    this.signal_eol();
                }
            }
        });
    }

    fn fetch_source_outputs(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_source_output_info_list(move |result: ListResult<&SourceOutputInfo>| {
            match result {
                ListResult::Item(info) => {
                    let owned = info.to_owned();
                    this.state.borrow_mut().source_outputs.insert(owned.index, owned);
                }
                ListResult::End => {
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch: source-output list error");
                    this.signal_eol();
                }
            }
        });
    }

    fn fetch_modules(self: &Rc<Self>) {
        self.pending.set(self.pending.get() + 1);
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_module_info_list(move |result: ListResult<&ModuleInfo>| {
            match result {
                ListResult::Item(info) => {
                    let owned = info.to_owned();
                    this.state.borrow_mut().modules.insert(owned.index, owned);
                }
                ListResult::End => {
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch: module list error");
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
                    let owned = info.to_owned();
                    this.state.borrow_mut().clients.insert(owned.index, owned);
                }
                ListResult::End => {
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch: client list error");
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
                    let owned = info.to_owned();
                    this.state.borrow_mut().samples.insert(owned.index, owned);
                }
                ListResult::End => {
                    this.signal_eol();
                }
                ListResult::Error => {
                    eprintln!("fetch: sample list error");
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
            let owned = server_info.to_owned();
            this.state.borrow_mut().server = Some(owned);
            this.signal_eol();
        });
    }

    // ── per-entity fetches (by-index) ────────────────────────────────

    fn fetch_sink_by_index(self: &Rc<Self>, idx: u32) {
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_sink_info_by_index(idx, move |result: ListResult<&SinkInfo>| {
            if let ListResult::Item(info) = result {
                let owned = info.to_owned();
                this.state.borrow_mut().sinks.insert(owned.index, owned);
            } else if let ListResult::End = result {
                this.state.borrow().print_facility(Facility::Sink, idx);
            }
        });
    }

    fn fetch_sink_input_by_index(self: &Rc<Self>, idx: u32) {
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_sink_input_info(idx, move |result: ListResult<&SinkInputInfo>| {
            if let ListResult::Item(info) = result {
                let owned = info.to_owned();
                this.state.borrow_mut().sink_inputs.insert(owned.index, owned);
            } else if let ListResult::End = result {
                this.state.borrow().print_facility(Facility::SinkInput, idx);
            }
        });
    }

    fn fetch_source_by_index(self: &Rc<Self>, idx: u32) {
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_source_info_by_index(idx, move |result: ListResult<&SourceInfo>| {
            if let ListResult::Item(info) = result {
                let owned = info.to_owned();
                this.state.borrow_mut().sources.insert(owned.index, owned);
            } else if let ListResult::End = result {
                this.state.borrow().print_facility(Facility::Source, idx);
            }
        });
    }

    fn fetch_module_by_index(self: &Rc<Self>, idx: u32) {
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_module_info(idx, move |result: ListResult<&ModuleInfo>| {
            if let ListResult::Item(info) = result {
                let owned = info.to_owned();
                this.state.borrow_mut().modules.insert(owned.index, owned);
            } else if let ListResult::End = result {
                this.state.borrow().print_facility(Facility::Module, idx);
            }
        });
    }

    fn fetch_client_by_index(self: &Rc<Self>, idx: u32) {
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_client_info(idx, move |result: ListResult<&ClientInfo>| {
            if let ListResult::Item(info) = result {
                let owned = info.to_owned();
                this.state.borrow_mut().clients.insert(owned.index, owned);
            } else if let ListResult::End = result {
                this.state.borrow().print_facility(Facility::Client, idx);
            }
        });
    }

    fn fetch_card_by_index(self: &Rc<Self>, idx: u32) {
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_card_info_by_index(idx, move |result: ListResult<&CardInfo>| {
            if let ListResult::Item(info) = result {
                let owned = info.to_owned();
                this.state.borrow_mut().cards.insert(owned.index, owned);
            } else if let ListResult::End = result {
                this.state.borrow().print_facility(Facility::Card, idx);
            }
        });
    }

    fn fetch_sample_by_index(self: &Rc<Self>, idx: u32) {
        let this = Rc::clone(self);
        let introspector = self.context.borrow().introspect();
        let _op = introspector.get_sample_info_by_index(idx, move |result: ListResult<&SampleInfo>| {
            if let ListResult::Item(info) = result {
                let owned = info.to_owned();
                this.state.borrow_mut().samples.insert(owned.index, owned);
            } else if let ListResult::End = result {
                this.state.borrow().print_facility(Facility::SampleCache, idx);
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
    pub fn subscribe(self: &Rc<Self>) {
        self.context
            .borrow_mut()
            .subscribe(InterestMaskSet::all(), |_success| {});

        self.context
            .borrow_mut()
            .set_subscribe_callback(Some(Self::make_event_callback(self)));
    }

    /// Print the current state as JSON to stdout.
    pub fn print_state(&self) {
        self.state.borrow().print_full_state();
    }

    /// Factory – produces the boxed event callback.
    fn make_event_callback(this: &Rc<Self>) -> Box<dyn FnMut(Option<Facility>, Option<Operation>, u32)> {
        let this = Rc::clone(this);
        Box::new(move |facility, operation, idx| {
            let f = facility.map(facility_name).unwrap_or("unknown");
            let o = operation.map(operation_name).unwrap_or("unknown");
            match operation {
                Some(Operation::Removed) => {
                    println!(r#"{{"index":{},"event":"remove","on":"{}"}}"#, idx, f);
                }
                Some(Operation::New) => {
                    let this_cb = Rc::clone(&this);
                    match facility {
                        Some(Facility::Sink) => this_cb.fetch_sink_by_index(idx),
                        Some(Facility::Source) => this_cb.fetch_source_by_index(idx),
                        Some(Facility::SinkInput) => this_cb.fetch_sink_input_by_index(idx),
                        Some(Facility::SourceOutput) => this_cb.fetch_source_by_index(idx),
                        Some(Facility::Module) => this_cb.fetch_module_by_index(idx),
                        Some(Facility::Client) => this_cb.fetch_client_by_index(idx),
                        Some(Facility::SampleCache) => this_cb.fetch_sample_by_index(idx),
                        Some(Facility::Card) => this_cb.fetch_card_by_index(idx),
                        Some(Facility::Server) => this_cb.fetch_server(),
                        None => {}
                    }
                }
                _ => {
                    println!("[{}] {} idx={}", o, f, idx);
                }
            }
        })
    }
}