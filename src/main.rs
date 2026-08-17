//! PulseAudio event monitor — subscribes to all PulseAudio events and prints
//! each event's index, facility, and type to stdout.

mod listener;

use std::cell::RefCell;
use std::ops::Deref;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use libpulse_binding as pulse;
use pulse::context::{Context, FlagSet as ContextFlagSet, State as ContextState};
use pulse::mainloop::signal::{Event as SignalEvent, MainloopSignals};
use pulse::mainloop::standard::{IterateResult, Mainloop};

use listener::PulseEventListener;

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

    // ── listener ──────────────────────────────────────────────────────
    let listener = PulseEventListener::new(Rc::clone(&context));

    // ── flag: has fetch_all been fired? ───────────────────────────────
    let mut fetch_started = false;

    // ── signal handling (SIGINT) ──────────────────────────────────────
    mainloop
        .borrow_mut()
        .init_signals()
        .expect("Failed to initialise signal subsystem");

    let quit_requested = Rc::new(AtomicBool::new(false));

    // Register the SIGINT handler.  The callback sets an atomic flag
    // instead of calling mainloop.quit() directly, because the signal
    // fires synchronously within mainloop.iterate() which already holds
    // a mutable borrow on the mainloop.
    {
        let q = Rc::clone(&quit_requested);
        let _sigint = SignalEvent::new(libc::SIGINT, move |_sig| {
            eprintln!("\n[SIGINT] shutting down...");
            q.store(true, Ordering::SeqCst);
        });

        // ── event loop ────────────────────────────────────────────────
        loop {
            match mainloop.borrow_mut().iterate(false) {
                IterateResult::Quit(_) | IterateResult::Err(_) => return,
                IterateResult::Success(_) => {}
            }

            // Check for SIGINT after each iteration.
            if quit_requested.load(Ordering::SeqCst) {
                return;
            }

            let state = context.borrow().get_state();
            match state {
                ContextState::Ready => {
                    if !fetch_started {
                        fetch_started = true;
                        let this = Rc::clone(&listener);
                        listener.fetch_all(move || {
                            this.subscribe();
                            eprintln!("Listening for events... (Ctrl+C to quit)");
                        });
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