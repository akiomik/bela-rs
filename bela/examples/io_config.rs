//! Hardware probe for how a board configures its analog and digital
//! I/O.
//!
//! Written to answer the first half of #11. The analog and digital
//! accessors on the context types have only ever run against hand-built
//! contexts, and two of the claims their documentation rests on — that
//! a Gem's analog outputs are its audio outputs with a +2 channel
//! offset, and that `uniformSampleRate` is on by default — come from
//! Bela's migration guide rather than from a measurement. Both are
//! about numbers a board reports, so both can be settled before any
//! wire is connected. `scripts/probe-io.sh` drives this, and what it
//! finds belongs in `docs/board-facts.md`.
//!
//! Nothing here reads or writes a sample. Confirming that a pin does
//! what the accessor says needs a voltage on it, which is the other
//! half of #11 and a different instrument.
//!
//! It is not a check with a right answer, so it is not part of
//! `scripts/smoke-test.sh`: what it prints is the board's description
//! of itself, and a pass/fail gate has nothing to compare that against
//! until the facts are written down.
//!
//! # What each run does
//!
//! - `hardware` — what libbela says the board is, before any audio
//!   system exists: the detected board and the version of the library
//!   that detected it, the `BelaHwConfig` that hardware implies, and
//!   the analog and digital fields of `Bela_defaultSettings`. The only
//!   probe here that brings nothing up, so it is also the only one
//!   whose answer cannot depend on the settings it was asked for.
//! - `context [options]` — brings one audio system up with the options
//!   given, reports the `BelaContext` that `setup` sees and the one the
//!   first block sees, renders for a moment and tears it down. The
//!   options are the ones that decide the shape of the block:
//!
//!   ```text
//!   --period <frames>        --uniform on|off
//!   --analog on|off          --digital on|off
//!   --analog-in <channels>   --analog-out <channels>
//!   --digital-channels <channels>
//!   ```
//!
//!   Unset options are left to `Bela_defaultSettings`, which is the
//!   point: the run with no options at all is the one that says what a
//!   board does when nobody asks for anything.
//!
//! # Both ends of the block are reported
//!
//! `setup` and the first block are asked the same questions because
//! they need not give the same answers: `setup` runs inside
//! `Bela_initAudio`, before an audio thread exists, and a frame count
//! that is only filled in once one does would show up as a difference
//! between the two. The accessors are documented as if there were no
//! difference, so a difference is a finding.
//!
//! # One configuration per process
//!
//! A failed `Bela_initAudio` poisons the process it happened in
//! (`docs/board-facts.md`), and every configuration here is one that
//! might fail. Each run therefore takes one configuration and exits,
//! and the driving script starts a fresh process for the next.
//!
//! Cross-compile and run on the board (see docs/cross-compile.md):
//!
//! ```sh
//! cargo build -p bela --release --target aarch64-unknown-linux-gnu --example io_config
//! ```

#![cfg_attr(
    not(bela_device),
    allow(
        dead_code,
        reason = "only the fallback main is reachable off-device; the probe code should still compile and lint"
    )
)]

use std::process::ExitCode;

use bela::{BelaApplication, BlockContext, RenderContext, SetupContext, ThreadInfo, rt_println};

/// Reports the shape of the block from both ends of it.
struct Report {
    /// Whether the next block is the first one. The report is per run,
    /// not per block: at 2760 blocks a second the second one would only
    /// bury the first.
    first_block: bool,
}

impl BelaApplication for Report {
    type RenderState = ();

    fn setup(&mut self, context: &SetupContext) -> bool {
        // `println!`, not `rt_println!`: `setup` runs inside
        // `Bela_initAudio` before there is an audio thread, and a
        // flushed write is what a script reading this over ssh needs.
        // The block report below is on the audio thread and cannot use
        // it.
        report_context("setup", context);
        true
    }

    fn create_render_state(&mut self, _thread: ThreadInfo, _context: &SetupContext) {}

    fn render_pre(&mut self, _states: &mut [()], context: &mut BlockContext) {
        if !self.first_block {
            return;
        }
        self.first_block = false;
        // Bela's own real-time printing, because this is the audio
        // thread. One key per line: a message is capped at
        // `MESSAGE_CAPACITY` bytes and a truncated one would read as a
        // missing field.
        rt_println!(
            "io-config: block-audio=frames:{},in:{},out:{},rate:{}",
            context.audio_frames(),
            context.audio_in_channels(),
            context.audio_out_channels(),
            context.audio_sample_rate()
        );
        rt_println!(
            "io-config: block-analog=frames:{},in:{},out:{},rate:{}",
            context.analog_frames(),
            context.analog_in_channels(),
            context.analog_out_channels(),
            context.analog_sample_rate()
        );
        rt_println!(
            "io-config: block-digital=frames:{},channels:{},rate:{}",
            context.digital_frames(),
            context.digital_channels(),
            context.digital_sample_rate()
        );
        rt_println!(
            "io-config: block-threads=this:{},count:{},counted:{}",
            context.as_sys().thisThread,
            context.as_sys().threadCount,
            context.thread_count()
        );
    }

    // Silence: this probe is about how the audio system was configured,
    // not about what it renders.
    fn render(&self, _state: &mut (), _context: &mut RenderContext) {}
}

/// Reports one step, on the spot.
///
/// Flushed, because stdout is a pipe when the driving script is
/// listening and an audio system that goes on to fail should not take
/// what was already established with it.
fn report(key: &str, value: &str) {
    use std::io::{self, Write};

    println!("io-config: {key}={value}");
    let _ = io::stdout().flush();
}

/// The whole shape of the block, as `setup` sees it.
///
/// Split across three lines by domain, so that a run with analog or
/// digital disabled shows a line of zeros rather than a gap.
fn report_context(phase: &str, context: &SetupContext) {
    report(
        &format!("{phase}-audio"),
        &format!(
            "frames:{},in:{},out:{},rate:{}",
            context.audio_frames(),
            context.audio_in_channels(),
            context.audio_out_channels(),
            context.audio_sample_rate()
        ),
    );
    report(
        &format!("{phase}-analog"),
        &format!(
            "frames:{},in:{},out:{},rate:{}",
            context.analog_frames(),
            context.analog_in_channels(),
            context.analog_out_channels(),
            context.analog_sample_rate()
        ),
    );
    report(
        &format!("{phase}-digital"),
        &format!(
            "frames:{},channels:{},rate:{}",
            context.digital_frames(),
            context.digital_channels(),
            context.digital_sample_rate()
        ),
    );
    // Raw as well as counted. `thread_count()` reads a `threadCount`
    // of 0 as 1, which is a `BelaContext`'s other way of spelling one
    // render thread — so the crate's number cannot say which of the
    // two libbela wrote, and a record of board behaviour wants the
    // field as it stands.
    report(
        &format!("{phase}-threads"),
        &format!(
            "this:{},count:{},counted:{}",
            context.as_sys().thisThread,
            context.as_sys().threadCount,
            context.thread_count()
        ),
    );
}

#[cfg(bela_device)]
mod probes {
    use core::time::Duration;
    use std::thread;

    use bela::bela_sys::{
        Bela_HwConfig_delete, Bela_HwConfig_new, Bela_InitSettings_alloc, Bela_InitSettings_free,
        Bela_defaultSettings,
    };
    use bela::{Bela, Board, DetectMode, Settings, Version};

    use super::{Report, report};

    /// How long the audio system renders before it is torn down. Long
    /// enough that a working one reports thousands of blocks, so that
    /// "came up but never rendered" cannot be mistaken for it.
    const RENDER_TIME: Duration = Duration::from_secs(1);

    /// Reads the cached detection rather than scanning.
    ///
    /// `DetectMode::Cache` is what the daemon leaves behind in
    /// `/run/bela/belaconfig`, and a scan would go out over I²C to find
    /// out something this probe only wants reported.
    const DETECT_CACHED: DetectMode = DetectMode::Cache;

    /// The board with the number it is, so that the record says
    /// `GemStereo(2)` rather than either on its own.
    ///
    /// A value the crate has no name for is worth more than "unknown":
    /// it means the board's libbela knows a hardware the vendored
    /// headers do not, and [`Board::Unrecognised`] prints as
    /// `unrecognised(<n>)` rather than losing the number.
    fn hardware_name(board: Board) -> String {
        if board.is_recognised() {
            format!("{board}({raw})", raw = board.to_sys())
        } else {
            board.to_string()
        }
    }

    /// What the board is, and what libbela expects of it, with no audio
    /// system anywhere in the picture.
    pub(crate) fn hardware() {
        let board = Board::detect(DETECT_CACHED);
        report("detect-hw", &hardware_name(board));
        // The library that answered, which is not necessarily the one
        // this was built against: every number below is a claim about a
        // particular libbela, and this is which one.
        report("version", &Version::running().to_string());
        let hw = board.to_sys();

        // The configuration libbela associates with that hardware,
        // which is where a Gem's channel counts come from before any
        // settings are applied. Null is an answer too: it is what a
        // hardware libbela has no configuration for looks like.
        let config = unsafe { Bela_HwConfig_new(hw) };
        if config.is_null() {
            report("hw-config", "null");
        } else {
            let config_ref = unsafe { &*config };
            report(
                "hw-config",
                &format!(
                    "rate:{},audio-in:{},audio-out:{},analog-in:{},analog-out:{},digital:{}",
                    config_ref.audioSampleRate,
                    config_ref.audioInChannels,
                    config_ref.audioOutChannels,
                    config_ref.analogInChannels,
                    config_ref.analogOutChannels,
                    config_ref.digitalChannels
                ),
            );
            unsafe { Bela_HwConfig_delete(config) };
        }

        // The defaults an application inherits by setting nothing,
        // which is what `Settings`'s "unset fields keep the values
        // produced by `Bela_defaultSettings()`" means in practice.
        let raw = unsafe { Bela_InitSettings_alloc() };
        if raw.is_null() {
            report("defaults", "alloc-failed");
            return;
        }
        unsafe { Bela_defaultSettings(raw) };
        let defaults = unsafe { &*raw };
        report(
            "defaults-analog",
            &format!(
                "use:{},in:{},out:{},uniform:{}",
                defaults.useAnalog,
                defaults.numAnalogInChannels,
                defaults.numAnalogOutChannels,
                defaults.uniformSampleRate
            ),
        );
        report(
            "defaults-digital",
            &format!(
                "use:{},channels:{}",
                defaults.useDigital, defaults.numDigitalChannels
            ),
        );
        report(
            "defaults-audio",
            &format!(
                "period:{},rate:{},threads:{}",
                defaults.periodSize, defaults.audioSampleRate, defaults.threadCount
            ),
        );
        unsafe { Bela_InitSettings_free(raw) };
    }

    /// One audio system, one configuration, both ends of one block.
    pub(crate) fn context(settings: &Settings) {
        let app = Report { first_block: true };
        let mut bela = match Bela::new(app, settings) {
            Ok(bela) => bela,
            Err(error) => {
                // A configuration the board will not have is a finding,
                // not a failure of the probe: which combinations are
                // refused is part of what #11 asks.
                report("init", &format!("failed-{error:?}"));
                return;
            }
        };
        report("init", "created");
        if let Err(error) = bela.start() {
            report("start", &format!("failed-{error:?}"));
            return;
        }
        report("start", "started");
        thread::sleep(RENDER_TIME);
        drop(bela);
        report("run", "stopped");
    }
}

/// Turns the command line into the settings to bring an audio system up
/// with.
///
/// Every option is optional and an unset one is left to
/// `Bela_defaultSettings`, so `context` with no options is a valid run
/// and the interesting one.
#[cfg(bela_device)]
fn parse_settings(arguments: &[String]) -> Option<bela::Settings> {
    fn switch(value: &str) -> Option<bool> {
        match value {
            "on" => Some(true),
            "off" => Some(false),
            _ => None,
        }
    }

    let mut settings = bela::Settings::new();
    let mut rest = arguments.iter();
    while let Some(option) = rest.next() {
        let value = rest.next()?;
        settings = match option.as_str() {
            "--period" => settings.period_size(value.parse().ok()?),
            "--uniform" => settings.uniform_sample_rate(switch(value)?),
            "--analog" => settings.use_analog(switch(value)?),
            "--digital" => settings.use_digital(switch(value)?),
            "--analog-in" => settings.num_analog_in_channels(value.parse().ok()?),
            "--analog-out" => settings.num_analog_out_channels(value.parse().ok()?),
            "--digital-channels" => settings.num_digital_channels(value.parse().ok()?),
            _ => return None,
        };
    }
    Some(settings)
}

#[cfg(bela_device)]
fn main() -> ExitCode {
    use std::env::args;

    let arguments: Vec<String> = args().skip(1).collect();
    match arguments.split_first() {
        Some((probe, rest)) if probe == "hardware" && rest.is_empty() => probes::hardware(),
        Some((probe, rest)) if probe == "context" => {
            let Some(settings) = parse_settings(rest) else {
                eprintln!("context: cannot read the options {rest:?}");
                return ExitCode::FAILURE;
            };
            // Echoed so that a log read on its own says which run it is.
            let asked_for = if rest.is_empty() {
                "defaults".to_owned()
            } else {
                rest.join(" ")
            };
            report("settings", &asked_for);
            probes::context(&settings);
        }
        _ => {
            eprintln!(
                "usage: io_config (hardware | context [options])\n\
                 \x20 options: --period <frames> | --uniform on|off | --analog on|off\n\
                 \x20          | --digital on|off | --analog-in <channels>\n\
                 \x20          | --analog-out <channels> | --digital-channels <channels>\n\
                 one configuration per run: a failed initialisation poisons its process"
            );
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}

#[cfg(not(bela_device))]
fn main() -> ExitCode {
    eprintln!("This example must be cross-compiled for Bela Gem (aarch64-unknown-linux-gnu).");
    ExitCode::FAILURE
}
