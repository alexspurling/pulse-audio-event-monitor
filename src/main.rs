//! PulseAudio event monitor — subscribes to all PulseAudio events and prints
//! each event's index, facility, and type to stdout.

use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;

use libpulse_binding as pulse;
use pulse::context::subscribe::{Facility, InterestMaskSet, Operation};
use pulse::context::{Context, FlagSet as ContextFlagSet, State as ContextState};
use pulse::def::Retval;
use pulse::mainloop::signal::{Event as SignalEvent, MainloopSignals};
use pulse::mainloop::standard::{IterateResult, Mainloop};

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

fn operation_name(o: Operation) -> &'static str {
    match o {
        Operation::New => "new",
        Operation::Changed => "change",
        Operation::Removed => "remove",
    }
}

fn main() {
    // ── mainloop ──────────────────────────────────────────────────────
    let mainloop = Rc::new(RefCell::new(
        Mainloop::new().expect("Failed to create mainloop"),
    ));

    // ── context ───────────────────────────────────────────────────────
    let context = Rc::new(RefCell::new(
        Context::new(mainloop.borrow().deref(), "pa-event-monitor")
            .expect("Failed to create context"),
    ));

    context
        .borrow_mut()
        .connect(None, ContextFlagSet::NOFLAGS, None)
        .expect("Failed to connect to PulseAudio");

    // ── signal handling (SIGINT) ──────────────────────────────────────
    mainloop
        .borrow_mut()
        .init_signals()
        .expect("Failed to initialise signal subsystem");

    {
        let ml = Rc::clone(&mainloop);
        let _sigint = SignalEvent::new(libc::SIGINT, move |_sig| {
            eprintln!("\n[SIGINT] shutting down...");
            ml.borrow_mut().quit(Retval(0));
        });

        // ── poll until context is READY, then subscribe ────────────────
        let mut subscribed = false;
        loop {
            match mainloop.borrow_mut().iterate(false) {
                IterateResult::Quit(_) | IterateResult::Err(_) => return,
                IterateResult::Success(_) => {}
            }

            let state = context.borrow().get_state();
            match state {
                ContextState::Ready => {
                    if !subscribed {
                        subscribed = true;

                        // Subscribe to all event types.
                        // The returned Operation is dropped immediately (matching the C++
                        // pattern of pa_operation_unref after subscribe).
                        context
                            .borrow_mut()
                            .subscribe(InterestMaskSet::all(), |_success| {});

                        // Set the per-event callback.
                        context
                            .borrow_mut()
                            .set_subscribe_callback(Some(Box::new(
                                |facility: Option<Facility>,
                                 operation: Option<Operation>,
                                 idx: u32| {
                                    let f = facility
                                        .map(facility_name)
                                        .unwrap_or("unknown");
                                    let o = operation
                                        .map(operation_name)
                                        .unwrap_or("unknown");
                                    println!("[{}] {} idx={}", o, f, idx);
                                },
                            )));

                        eprintln!("Listening for events... (Ctrl+C to quit)");
                    }
                }
                ContextState::Failed | ContextState::Terminated => {
                    eprintln!("Connection failed or terminated");
                    return;
                }
                _ => {}
            }
        }
    }
}