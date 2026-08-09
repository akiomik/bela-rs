use core::fmt;
use std::error;

/// What `getopt` returns for an option it does not know, or one whose
/// value is missing.
const UNRECOGNISED_OPTION: i32 = b'?' as i32;

/// Errors returned by the Bela audio system lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Error {
    /// `Bela_initAudio` failed with the contained return code.
    ///
    /// The initialisation it failed partway through is not undone, so
    /// this is fatal to the process rather than to the one attempt:
    /// every later [`Bela::new`](crate::Bela::new) returns
    /// [`AudioSystemPoisoned`](Self::AudioSystemPoisoned).
    Init(i32),
    /// `Bela_startAudio` failed with the contained return code.
    Start(i32),
    /// The run ended with the contained number of callbacks refused
    /// for breaking the protocol the render states rely on.
    ///
    /// libbela made a callback somewhere the crate could not hand out
    /// the references [`BelaApplication`](crate::BelaApplication)
    /// promises — several `render` calls with the same thread number,
    /// or a `render_post` arriving while one was still in flight, which
    /// a stop requested mid-block can produce. Each such callback was
    /// skipped and a stop requested, so the audio that was rendered is
    /// sound and the run ended early rather than going wrong.
    ///
    /// Reported by [`Bela::until_stopped`](crate::Bela::until_stopped)
    /// and the `run` methods built on it, so that a run which ended
    /// this way is not mistaken for one that was asked to stop. See
    /// [`Bela::callback_faults`](crate::Bela::callback_faults).
    CallbackFaults(u32),
    /// An auxiliary task name contained a NUL byte.
    TaskName,
    /// `Bela_createAuxiliaryTask` failed, or the crate was built for a
    /// target with no audio system to create the task in.
    TaskCreate,
    /// An auxiliary task was created while an audio system was being
    /// torn down, which would have deleted it again immediately.
    ///
    /// This is what a `cleanup` callback gets: it runs inside that
    /// teardown.
    TaskCreateWhileStopping,
    /// `Bela_cpuMonitoringInit` failed.
    CpuMonitoring,
    /// The requested CPU monitoring acquisition cycle does not fit in a
    /// C `int`, which is how libbela takes it.
    CpuMonitoringCycle(u32),
    /// CPU monitoring was requested with a period size big enough that
    /// libbela runs `render` on a different thread from the one it
    /// measures.
    ///
    /// See
    /// [`MAX_MONITORED_PERIOD_SIZE`](crate::MAX_MONITORED_PERIOD_SIZE).
    CpuMonitoringPeriodSize(i32),
    /// Another [`Bela`](crate::Bela) audio system already exists in
    /// this process.
    ///
    /// The C API is a process-wide singleton, so a second one would
    /// share — and reset — the state the first is using.
    AudioSystemExists,
    /// An earlier `Bela_initAudio` in this process failed partway
    /// through, and no audio system can be built after that.
    ///
    /// libbela is left believing the audio system is up and offers no
    /// way to put it back: `Bela_cleanupAudio` segfaults on that path.
    /// So this is refused rather than attempted — going ahead means a
    /// segfault inside libbela, which is what the error replaces.
    ///
    /// Terminal for the process, and only for the process: the board is
    /// untouched, so a new one gets a working audio system straight
    /// away. See [`Bela::new`](crate::Bela::new).
    AudioSystemPoisoned,
    /// An argument was not one of Bela's standard command-line options.
    ///
    /// Carries what `Bela_getopt_long` returned: `'?'` for an
    /// unrecognised option or one missing its value, which `getopt` has
    /// already reported on standard error naming the argument, or an
    /// internal option code when libbela rejected a standard option it
    /// did recognise — a `--json-file` it could not read, say.
    CommandLine(i32),
    /// A command-line argument contained a NUL byte, which a C string
    /// cannot carry.
    CommandLineNul,
    /// The settings resolved to an audio sample rate of zero, which
    /// libbela reports as a codec that is not enabled.
    ///
    /// Zero is the only rate refused here, which is why this carries
    /// nothing: `--sample-rate` reads its value with `atof`, so
    /// anything that is not a number arrives as 0, and a negative rate
    /// is clamped to the same 0 by the parser. The message libbela
    /// prints for it — `Error: audio sampling rate is 0. Is the codec
    /// enabled?`, followed by one about a cape — describes hardware for
    /// a number the command line supplied.
    ///
    /// One of the checks [`Bela::new_with_args`](crate::Bela::new_with_args)
    /// makes before `Bela_initAudio`, where this would otherwise be an
    /// [`Init`](Self::Init) that costs the process every later audio
    /// system.
    SampleRate,
    /// The settings named a PRU other than 0 or 1, which are the two
    /// libbela can run the audio code on.
    ///
    /// Checked before `Bela_initAudio`, which refuses the same values
    /// and leaves the process unable to build another audio system.
    PruNumber(i32),
    /// The settings asked for a number of multiplexer channels libbela
    /// does not take: it accepts 0, which is off, and 2, 4 or 8.
    ///
    /// Checked before `Bela_initAudio`; see
    /// [`Bela::new_with_args`](crate::Bela::new_with_args) for what the
    /// crate does and does not check about `--mux-channels`.
    MultiplexerChannels(i32),
    /// The multiplexer was asked for while the audio code was pointed
    /// at a PRU other than 1, carried here.
    ///
    /// PRU 0 is a valid setting on its own; only the multiplexer needs
    /// PRU 1, which is why the two are separate errors.
    MultiplexerPru(i32),
    /// The multiplexer was asked for with the analog inputs disabled.
    ///
    /// This is the combination that gets past every check libbela makes
    /// on the ARM side and dies in the PRU firmware instead — `Invalid
    /// PRU configuration settings`, `PRU timeout`, `McASP error,
    /// abort`, with the process ending from inside libbela and nothing
    /// returned to the caller. Refusing it beforehand is the only place
    /// it can be reported at all.
    MultiplexerWithoutAnalog,
    /// The multiplexer was asked for with a number of analog input
    /// channels other than the 8 it needs, carried here.
    ///
    /// `--analog-channels` snaps what it is given to 8, 4 or 2, so this
    /// reports the number the settings resolved to rather than the one
    /// that was written on the command line. It is not only about
    /// asking for too few:
    /// [`Settings::num_analog_in_channels`](crate::Settings::num_analog_in_channels)
    /// is passed on as it stands, and 16 is as much a refusal as 4.
    ///
    /// Checked before `Bela_initAudio`, which refuses the same thing in
    /// the `BelaContextManager::setup` it calls and leaves the process
    /// unable to build another audio system.
    MultiplexerAnalogChannels(i32),
    /// `Bela_setLineOutLevel` failed with the contained return code,
    /// e.g. for a channel the codec does not have.
    LineOutLevel(i32),
    /// `Bela_setHpLevel` failed with the contained return code, e.g.
    /// for a channel the codec does not have.
    HeadphoneLevel(i32),
    /// `Bela_setAudioInputGain` failed with the contained return code.
    AudioInputGain(i32),
    /// `Bela_muteSpeakers` failed with the contained return code.
    MuteSpeakers(i32),
    /// A MIDI port name contained a NUL byte.
    MidiPortName,
    /// A `Midi` object could not be created, or the crate was built for
    /// a target with no `libbelaextra` to create one in.
    MidiCreate,
    /// A MIDI port could not be opened, with what the shim reported.
    ///
    /// [`bela_sys::BELA_MIDI_NO_SUCH_PORT`] for a name no port has —
    /// the names are the ones [`midi_ports`](crate::midi_ports) lists,
    /// which carry the subdevice — and a negative `errno` when ALSA
    /// refused the device itself, `-16` for a port something else
    /// already holds. The two are told apart rather than sharing `-1`,
    /// which would be the first of them and `EPERM`.
    ///
    /// [`bela_sys::BELA_MIDI_ALREADY_OPEN`] is the third value the
    /// shim can report and cannot arrive here: this crate opens each
    /// port on an object of its own, and drops it when the open fails.
    MidiOpen(i32),
    /// A MIDI value did not fit the type it was converted into.
    ///
    /// Carries what was given, the largest that type takes — 127 for a
    /// data byte, 15 for a channel, 16383 for a pitch bend — and what
    /// the type is. The wire has no room for more, and masking the
    /// extra bits off would turn a number that was wrong into a
    /// different number that is not.
    ///
    /// The name is there because a note and a velocity are two numbers
    /// in the same range, which is the whole reason they are separate
    /// types: an error that only said "127" would put them back
    /// together.
    MidiValue {
        /// What was given.
        value: u16,
        /// The largest the type takes.
        max: u16,
        /// What the type holds, as it reads in a sentence — `"note
        /// number"`, `"velocity"`, `"channel"`.
        kind: &'static str,
    },
    /// A render thread queued more MIDI messages between drains than
    /// [`MidiOutput::capacity`](crate::MidiOutput::capacity) allows.
    ///
    /// The message was not queued, and nothing else was affected. This
    /// says the program outran the budget it declared and nothing about
    /// the device: neither Bela's output pipe nor ALSA reports anything
    /// this crate could pass on.
    MidiQueueFull,
    /// MIDI was sent from a thread that cannot send it.
    ///
    /// [`MidiOutput::send`](crate::MidiOutput::send) and
    /// [`flush`](crate::MidiOutput::flush) write to Bela's pipe through
    /// an EVL out-of-band call, which only a thread EVL knows about can
    /// make. This crate's answer is the thread that opened the port: a
    /// write from any other reports success, delivers nothing, and
    /// leaves the output stream misaligned for the rest of the run, so
    /// it is refused instead.
    MidiThread,
    /// A level or gain was not a number of decibels libbela can convert
    /// into register values: not finite, or larger in magnitude than
    /// [`MAX_DECIBELS`](crate::MAX_DECIBELS).
    ///
    /// The conversion on the C side is a cast to `int`, which is
    /// undefined behaviour for those values, so they are refused before
    /// the call rather than passed on.
    Decibels,
}

impl fmt::Display for Error {
    #[allow(
        clippy::too_many_lines,
        reason = "one arm per variant, which is what makes a new variant fail to compile until \
                  it has a message; splitting the match would need a catch-all and lose that"
    )]
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Init(code) => write!(f, "Bela_initAudio failed with code {code}"),
            Self::Start(code) => write!(f, "Bela_startAudio failed with code {code}"),
            Self::CallbackFaults(faults) => write!(
                f,
                "{faults} callback(s) were refused for breaking the protocol the render states \
                 rely on, and the audio system was asked to stop"
            ),
            Self::TaskName => write!(f, "the auxiliary task name contains a NUL byte"),
            Self::TaskCreate => write!(f, "Bela_createAuxiliaryTask failed"),
            Self::TaskCreateWhileStopping => write!(
                f,
                "auxiliary tasks cannot be created while the audio system is stopping"
            ),
            Self::CpuMonitoring => write!(f, "Bela_cpuMonitoringInit failed"),
            Self::CpuMonitoringCycle(count) => write!(
                f,
                "the CPU monitoring cycle is {count} measurements, \
                 which does not fit in the C int libbela takes"
            ),
            Self::CpuMonitoringPeriodSize(frames) => write!(
                f,
                "CPU monitoring needs a period size of at most {max} frames, not {frames}: \
                 above that libbela renders on a separate thread from the one it measures",
                max = crate::MAX_MONITORED_PERIOD_SIZE
            ),
            Self::AudioSystemExists => write!(
                f,
                "a Bela audio system already exists in this process; the C API is a singleton"
            ),
            Self::AudioSystemPoisoned => write!(
                f,
                "an earlier Bela_initAudio failed in this process, leaving libbela with an audio \
                 system it will not give back; start a new process"
            ),
            Self::CommandLine(code) if *code == UNRECOGNISED_OPTION => write!(
                f,
                "an argument is not one of Bela's standard options, or is missing its value"
            ),
            Self::CommandLine(code) => write!(
                f,
                "the command line was rejected by Bela_getopt_long, which returned {code}"
            ),
            Self::CommandLineNul => {
                write!(f, "a command-line argument contains a NUL byte")
            }
            Self::SampleRate => write!(
                f,
                "the audio sample rate is 0, which libbela reports as a codec that is not enabled"
            ),
            Self::PruNumber(number) => {
                write!(f, "the audio code runs on PRU 0 or PRU 1, not PRU {number}")
            }
            Self::MultiplexerChannels(channels) => write!(
                f,
                "{channels} is not a number of multiplexer channels; \
                 the options are 0 for off, 2, 4 and 8"
            ),
            Self::MultiplexerPru(number) => write!(
                f,
                "the multiplexer runs on PRU 1, and the audio code was pointed at PRU {number}"
            ),
            Self::MultiplexerWithoutAnalog => write!(
                f,
                "the multiplexer needs the analog inputs, which are disabled; \
                 libbela checks neither and the PRU firmware ends the process"
            ),
            Self::MultiplexerAnalogChannels(channels) => write!(
                f,
                "the multiplexer needs 8 analog input channels, not {channels}"
            ),
            Self::LineOutLevel(code) => {
                write!(f, "Bela_setLineOutLevel failed with code {code}")
            }
            Self::HeadphoneLevel(code) => write!(f, "Bela_setHpLevel failed with code {code}"),
            Self::AudioInputGain(code) => {
                write!(f, "Bela_setAudioInputGain failed with code {code}")
            }
            Self::MuteSpeakers(code) => write!(f, "Bela_muteSpeakers failed with code {code}"),
            Self::MidiPortName => write!(f, "the MIDI port name contains a NUL byte"),
            Self::MidiCreate => write!(f, "a Bela Midi object could not be created"),
            Self::MidiOpen(code) if *code == bela_sys::BELA_MIDI_NO_SUCH_PORT => write!(
                f,
                "no MIDI port has that name; it has to be one of those listed by midi_ports, \
                 which carry the subdevice, as in hw:0,0,0"
            ),
            Self::MidiOpen(code) if *code == bela_sys::BELA_MIDI_ALREADY_OPEN => {
                write!(f, "that direction of this MIDI port is already open")
            }
            Self::MidiOpen(code) => write!(
                f,
                "the MIDI port could not be opened ({code}), which is -errno as ALSA reported it"
            ),
            Self::MidiValue { value, max, kind } => {
                write!(f, "{value} is more than the {max} a {kind} carries")
            }
            Self::MidiQueueFull => write!(
                f,
                "this render thread's MIDI output queue is full; it holds what the program asked \
                 MidiOutput::open for, per drain"
            ),
            Self::MidiThread => write!(
                f,
                "MIDI can only be sent from the thread that opened the port; a write from any \
                 other thread is lost and misaligns the output stream"
            ),
            Self::Decibels => write!(
                f,
                "a level must be a finite number of decibels of at most {max} in magnitude, \
                 which is what libbela can convert into register values",
                max = crate::MAX_DECIBELS
            ),
        }
    }
}

impl error::Error for Error {}
