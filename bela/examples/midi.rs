//! A MIDI monosynth that also echoes what it hears.
//!
//! It opens one port for input and one for output — the same one
//! unless told otherwise — plays the last note it was sent, prints
//! every message it receives, and sends each one straight back out.
//! An all-notes-off leaves on every channel on the way down.
//!
//! This is the example of where MIDI goes in an application:
//!
//! - **input in `render_pre`**, because taking a message advances a
//!   read pointer with one reader, and because what the messages
//!   change is what `render` then plays;
//! - **output through a [`MidiSender`] kept in a render state**, one
//!   per render thread, so the queue behind it has one writer;
//! - **the closing message in `cleanup`**, through
//!   [`MidiOutput::send`], because after the last block there is no
//!   drain left to run.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example midi
//! scp target/aarch64-unknown-linux-gnu/release/examples/midi root@bela.local:
//! ssh -t root@bela.local './midi'                    # first port for both
//! ssh -t root@bela.local './midi hw:1,1,0 hw:1,2,0'  # in, then out
//! ```
//!
//! The two names in the second line are `snd-virmidi`'s, wired the way
//! `docs/midi.md` describes: what is sent to `hw:1,0,0` arrives at
//! `hw:1,1,0`, and what this echoes to `hw:1,2,0` arrives at
//! `hw:1,3,0`. Reading and writing the same side of that wiring would
//! feed the echo back into the input.
//!
//! With nothing attached, a Gem still has the USB gadget port
//! (`hw:0,0,0`), so it runs and waits. `bela::midi_ports()` is what
//! the names come from, and they are not the ones `amidi -l` prints:
//! Bela's carry the subdevice.

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the application code should still compile and lint"
    )
)]

use core::f32::consts::TAU;
#[cfg(bela_device)]
use std::env;
#[cfg(not(bela_device))]
use std::process::ExitCode;

use bela::{
    BelaApplication, BlockContext, CleanupContext, ControlValue, Controller, MidiChannel,
    MidiInput, MidiMessage, MidiOutput, MidiSender, Note, RenderContext, SetupContext, ThreadInfo,
    rt_println,
};

/// How many messages one render thread may queue between drains.
///
/// This one only echoes, so it queues what it receives: a block's
/// worth of a busy controller sweep, and no more.
const QUEUE_CAPACITY: usize = 16;

const AMPLITUDE: f32 = 0.3;

/// Controller 123: everything this channel is playing, off.
const ALL_NOTES_OFF: Controller = match Controller::new(123) {
    Some(controller) => controller,
    None => unreachable!(),
};

struct Monosynth {
    input: Option<MidiInput>,
    output: Option<MidiOutput>,
    in_port: String,
    out_port: String,
    /// The note being played, if any. A note off for a different one
    /// — a key released while a later one is still held — leaves it
    /// alone, which is the least a monosynth has to remember.
    playing: Option<Note>,
    /// What playing it comes to, per frame.
    phase_increment: f32,
    /// Where the block starts, as `sine.rs` carries it.
    phase: f32,
    sample_rate: f32,
    received: u64,
    echoed: u64,
    /// Echoes the queue had no room for. Nothing here can produce one
    /// — a block brings a handful of messages and the queue holds 16 —
    /// so a number other than zero is worth knowing about.
    dropped: u64,
}

/// One render thread's share of the work.
struct Voice {
    /// The first frame this thread writes, so a block's phase can be
    /// worked out for it.
    first_frame: usize,
    phase: f32,
    /// This thread's end of the output queue. Only thread 0's is used
    /// here — `render_pre` echoes, and that is one thread — but every
    /// thread takes its own, which is what a synthesiser sending from
    /// `render` would want.
    sender: Option<MidiSender>,
}

impl BelaApplication for Monosynth {
    type RenderState = Voice;

    fn setup(&mut self, context: &SetupContext) -> bool {
        self.sample_rate = context.audio_sample_rate();

        let ports = bela::midi_ports();
        if self.in_port.is_empty() {
            let Some(first) = ports.first() else {
                println!("no MIDI ports; nothing to open");
                return false;
            };
            self.in_port.clone_from(first);
        }
        if self.out_port.is_empty() {
            self.out_port.clone_from(&self.in_port);
        }

        match MidiInput::open(&self.in_port) {
            Ok(input) => self.input = Some(input),
            Err(error) => {
                println!("cannot read from {}: {error}", self.in_port);
                return false;
            }
        }
        match MidiOutput::open(&self.out_port, context, QUEUE_CAPACITY) {
            Ok(output) => self.output = Some(output),
            Err(error) => {
                println!("cannot write to {}: {error}", self.out_port);
                return false;
            }
        }

        println!(
            "setup: in {}, out {}, of {} port(s)",
            self.in_port,
            self.out_port,
            ports.len()
        );
        true
    }

    fn create_render_state(&mut self, thread: ThreadInfo, context: &SetupContext) -> Voice {
        Voice {
            first_frame: thread.frame_range(context.audio_frames()).start,
            phase: 0.0,
            // Once per thread: a sender is handed out exactly once, and
            // holding it here is what makes its queue single-writer.
            sender: self
                .output
                .as_mut()
                .and_then(|output| output.take_sender(thread.index())),
        }
    }

    // Real-time safe: ring reads, arithmetic, and a queue push per
    // message echoed. Nothing here allocates, blocks or waits.
    fn render_pre(&mut self, states: &mut [Voice], context: &mut BlockContext) {
        // `read` rather than `MidiInput::messages`, which is the
        // iterator this crate points at first: the body needs
        // `self.received` and `states`, and an iterator borrowing
        // `self.input` holds `self` for as long as the loop runs.
        if let Some(input) = self.input.as_mut() {
            while let Some(message) = input.read() {
                self.received += 1;
                rt_println!("{message:?}");
                match message {
                    // A note on with velocity 0 is how most devices end
                    // a note; a note off is the other way.
                    MidiMessage::NoteOn { note, velocity, .. } if velocity.get() > 0 => {
                        self.playing = Some(note);
                        self.phase_increment = increment(note, self.sample_rate);
                    }
                    MidiMessage::NoteOn { note, .. } | MidiMessage::NoteOff { note, .. }
                        if self.playing == Some(note) =>
                    {
                        self.playing = None;
                        self.phase_increment = 0.0;
                    }
                    _ => {}
                }
                // Straight back out, through thread 0's queue: this is
                // one thread, so one queue is the right one to use.
                let sent = states
                    .first_mut()
                    .and_then(|voice| voice.sender.as_mut())
                    .is_some_and(|sender| sender.send(context, message).is_ok());
                if sent {
                    self.echoed += 1;
                } else {
                    self.dropped += 1;
                }
            }
        }

        for state in states {
            #[allow(
                clippy::cast_precision_loss,
                reason = "a frame index within a block is far below f32's exact integer range"
            )]
            let offset = state.first_frame as f32 * self.phase_increment;
            state.phase = self.phase + offset;
        }
    }

    // Real-time safe: arithmetic and writes to this thread's frames.
    fn render(&self, state: &mut Voice, context: &mut RenderContext) {
        for frame in context.audio_frame_range() {
            let sample = if self.phase_increment > 0.0 {
                AMPLITUDE * state.phase.sin()
            } else {
                0.0
            };
            for channel in 0..context.audio_out_channels() {
                context.audio_write(frame, channel, sample);
            }
            state.phase += self.phase_increment;
        }
    }

    // Real-time safe: one multiplication and a wrap.
    fn render_post(&mut self, _states: &mut [Voice], context: &mut BlockContext) {
        #[allow(
            clippy::cast_precision_loss,
            reason = "a block's frame count is far below f32's exact integer range"
        )]
        let advanced = context.audio_frames() as f32 * self.phase_increment;
        self.phase = (self.phase + advanced) % TAU;
    }

    fn cleanup(&mut self, _states: &mut [Voice], _context: &CleanupContext) {
        // Nothing is rendering any more, so nothing will drain the
        // queues: this goes out on the calling thread instead, which is
        // the one that opened the port.
        if let Some(output) = self.output.as_mut() {
            // Every channel, because this echoes whatever channel it
            // was sent on and a note left playing on one it never
            // looked at is still a note left playing. Sixteen sends is
            // nothing here: audio has stopped, and this is the thread
            // that opened the port.
            for channel in 0..=MidiChannel::MAX.get() {
                let all_off = MidiMessage::ControlChange {
                    channel: MidiChannel::new(channel).expect("0..=15 is every channel"),
                    controller: ALL_NOTES_OFF,
                    value: ControlValue::MIN,
                };
                if let Err(error) = output.send(all_off) {
                    println!("cleanup: all notes off was not sent: {error}");
                    break;
                }
            }
        }
        println!(
            "cleanup: {} message(s) received, {} echoed, {} dropped",
            self.received, self.echoed, self.dropped
        );
    }
}

/// The phase increment per frame for `note`, at concert pitch.
fn increment(note: Note, sample_rate: f32) -> f32 {
    // MIDI note 69 is A4, and there are twelve semitones to an octave.
    let semitones = f32::from(note.get()) - 69.0;
    let frequency = 440.0 * (semitones / 12.0).exp2();
    TAU * frequency / sample_rate
}

#[cfg(bela_device)]
fn main() -> Result<(), bela::Error> {
    let mut arguments = env::args().skip(1);
    let synth = Monosynth {
        input: None,
        output: None,
        in_port: arguments.next().unwrap_or_default(),
        out_port: arguments.next().unwrap_or_default(),
        playing: None,
        phase_increment: 0.0,
        phase: 0.0,
        sample_rate: 0.0,
        received: 0,
        echoed: 0,
        dropped: 0,
    };
    bela::Bela::run(synth, &bela::Settings::new())
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
