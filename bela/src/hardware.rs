//! What the program is running on: the board, and the Bela version.
//!
//! Two questions a program can ask before it does anything else, and
//! the only two that can be answered without an audio system:
//! [`Board::detect`] wraps `Bela_detectHw` and [`Version::running`]
//! wraps `Bela_getVersion`. Neither touches the audio hardware, so
//! both can run in a `main` that has not called
//! [`Bela::new`](crate::Bela::new) yet — which is what makes them
//! useful for declining to start.
//!
//! ```ignore
//! use bela::{Board, DetectMode, Version};
//!
//! let board = Board::detect(DetectMode::Cache);
//! if board != Board::GemStereo {
//!     eprintln!("this program was measured on a Gem Stereo, not a {board}");
//! }
//! println!("libbela {} on {board}", Version::running());
//! ```
//!
//! # The list of boards is the image's, not this crate's
//!
//! `BelaHw` is a C enum and `Bela_detectHw` returns it as an `int`.
//! Which values that `int` can take is decided by the libbela the
//! program is linked against, and this crate's names for them come
//! from the headers vendored in `bela-sys` — a board added to a later
//! image is a value with no name here. [`Board::Unrecognised`] is
//! where those land, carrying the number, rather than being folded
//! into a board this crate does happen to know.
//!
//! # Which version is which
//!
//! [`Version::running`] asks the library, and [`Version::HEADERS`] is
//! what the vendored headers said when the bindings were generated.
//! They are the same number on a board whose image matches the
//! headers, and a program that prints both says so in one line when
//! they are not — which is the difference between a bug report that
//! names a mismatched image and one that does not.
//!
//! # On a Bela Gem Stereo
//!
//! Measured on the board (see `docs/board-facts.md`): it detects as
//! [`Board::GemStereo`] through every mode but
//! [`DetectMode::UserOnly`], which finds no `~/.bela/belaconfig` and
//! answers [`Board::NoHardware`], and the library reports version
//! 1.18.0.

use core::ffi::{c_int, c_uint};
use core::fmt;

use bela_sys::{
    BELA_BUGFIX_VERSION, BELA_MAJOR_VERSION, BELA_MINOR_VERSION, BelaHw, BelaHw_BelaHw_Batch,
    BelaHw_BelaHw_Bela, BelaHw_BelaHw_BelaEs9080, BelaHw_BelaHw_BelaMini,
    BelaHw_BelaHw_BelaMiniMultiAudio, BelaHw_BelaHw_BelaMiniMultiI2s,
    BelaHw_BelaHw_BelaMiniMultiTdm, BelaHw_BelaHw_BelaMultiTdm, BelaHw_BelaHw_BelaRevC,
    BelaHw_BelaHw_CtagBeast, BelaHw_BelaHw_CtagBeastBela, BelaHw_BelaHw_CtagFace,
    BelaHw_BelaHw_CtagFaceBela, BelaHw_BelaHw_GemMulti, BelaHw_BelaHw_GemStereo,
    BelaHw_BelaHw_NoHw, BelaHw_BelaHw_Salt, BelaHwDetectMode,
    BelaHwDetectMode_BelaHwDetectMode_Cache, BelaHwDetectMode_BelaHwDetectMode_CacheOnly,
    BelaHwDetectMode_BelaHwDetectMode_Scan, BelaHwDetectMode_BelaHwDetectMode_User,
    BelaHwDetectMode_BelaHwDetectMode_UserOnly,
};

/// A board, as libbela's `BelaHw` names it.
///
/// The spelling of each name is libbela's own, so that what a program
/// prints can be compared with what `/run/bela/belaconfig` holds —
/// `HARDWARE=GemStereo` on the board these bindings were measured
/// against.
///
/// Not exhaustive, and [`Unrecognised`](Self::Unrecognised) is not the
/// only reason: a board named by a later image is a new variant here
/// once the headers are re-vendored, and matching has to keep room for
/// it either way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Board {
    /// No hardware was detected (`BelaHw_NoHw`).
    ///
    /// What a detect mode that only reads a file answers when the file
    /// is not there, as well as what a board with nothing on it says.
    NoHardware,
    /// Bela (`BelaHw_Bela`).
    Bela,
    /// Bela Mini (`BelaHw_BelaMini`).
    BelaMini,
    /// Gem Stereo (`BelaHw_GemStereo`).
    GemStereo,
    /// Gem Multi (`BelaHw_GemMulti`).
    GemMulti,
    /// Salt (`BelaHw_Salt`).
    Salt,
    /// Ctag Face (`BelaHw_CtagFace`).
    CtagFace,
    /// Ctag Beast (`BelaHw_CtagBeast`).
    CtagBeast,
    /// Ctag Face and Bela cape (`BelaHw_CtagFaceBela`).
    CtagFaceBela,
    /// Ctag Beast and Bela cape (`BelaHw_CtagBeastBela`).
    CtagBeastBela,
    /// Bela Mini with extra codecs (`BelaHw_BelaMiniMultiAudio`).
    BelaMiniMultiAudio,
    /// Bela Mini with extra codecs and/or TDM devices
    /// (`BelaHw_BelaMiniMultiTdm`).
    BelaMiniMultiTdm,
    /// Bela with extra codecs and/or TDM devices
    /// (`BelaHw_BelaMultiTdm`).
    BelaMultiTdm,
    /// Bela Mini with extra RX and TX I²S data lines
    /// (`BelaHw_BelaMiniMultiI2s`).
    BelaMiniMultiI2s,
    /// A Bela cape with an ES9080 EVB on top, all as audio
    /// (`BelaHw_BelaEs9080`).
    BelaEs9080,
    /// A Bela cape rev C, where the ES9080 provides the analog outputs
    /// (`BelaHw_BelaRevC`).
    BelaRevC,
    /// The offline dummy hardware (`BelaHw_Batch`).
    Batch,
    /// A value the vendored headers do not name, kept as libbela
    /// returned it.
    ///
    /// This is a board added to `BelaHw` after the headers in
    /// `bela-sys/vendor` were taken, not a failure to detect — that is
    /// [`NoHardware`](Self::NoHardware). Worth reporting: it means the
    /// image knows hardware this crate has never been built against.
    ///
    /// Readable — `Board::Unrecognised { raw, .. }` matches — but not
    /// constructible from outside this crate, which is what the
    /// variant's `#[non_exhaustive]` buys. A hand-built
    /// `Unrecognised { raw: 2 }` would be a board that carries
    /// `GemStereo`'s number without being equal to
    /// [`GemStereo`](Self::GemStereo), so
    /// [`from_raw`](Self::from_raw) is the only way to make one and
    /// every board round-trips through it.
    ///
    /// ```compile_fail,E0639
    /// // A board that would answer 2 to `to_raw` while comparing
    /// // unequal to `Board::GemStereo`, which `from_raw(2)` is.
    /// let board = bela::Board::Unrecognised { raw: 2 };
    /// ```
    #[non_exhaustive]
    Unrecognised {
        /// The `BelaHw` libbela returned.
        raw: c_int,
    },
}

impl Board {
    /// Asks libbela what the program is running on.
    ///
    /// No audio system is involved, so this can run at the top of
    /// `main` and decide whether to build one at all. What each `mode`
    /// costs and reads is on [`DetectMode`] — they are not
    /// interchangeable, and [`DetectMode::Scan`] is the one with a side
    /// effect.
    ///
    /// Only available on the device target
    /// (`aarch64-unknown-linux-gnu`).
    #[cfg(bela_device)]
    #[must_use]
    pub fn detect(mode: DetectMode) -> Self {
        // Safety: `Bela_detectHw` takes a value of the C enum by copy
        // and returns another; it reads files and buses of its own and
        // borrows nothing from this side.
        Self::from_raw(unsafe { bela_sys::Bela_detectHw(mode.to_raw()) })
    }

    /// Reads a raw `BelaHw` as a board.
    ///
    /// For values arriving from somewhere other than [`detect`] — the
    /// `board` field of a `BelaInitSettings`, say, or a call made
    /// through [`bela_sys`] directly.
    ///
    /// [`detect`]: Self::detect
    #[must_use]
    pub const fn from_raw(raw: BelaHw) -> Self {
        // A chain rather than a `match`: the generated constants are
        // spelled in C's case, and a pattern made of them warns on
        // every arm.
        if raw == BelaHw_BelaHw_NoHw {
            Self::NoHardware
        } else if raw == BelaHw_BelaHw_Bela {
            Self::Bela
        } else if raw == BelaHw_BelaHw_BelaMini {
            Self::BelaMini
        } else if raw == BelaHw_BelaHw_GemStereo {
            Self::GemStereo
        } else if raw == BelaHw_BelaHw_GemMulti {
            Self::GemMulti
        } else if raw == BelaHw_BelaHw_Salt {
            Self::Salt
        } else if raw == BelaHw_BelaHw_CtagFace {
            Self::CtagFace
        } else if raw == BelaHw_BelaHw_CtagBeast {
            Self::CtagBeast
        } else if raw == BelaHw_BelaHw_CtagFaceBela {
            Self::CtagFaceBela
        } else if raw == BelaHw_BelaHw_CtagBeastBela {
            Self::CtagBeastBela
        } else if raw == BelaHw_BelaHw_BelaMiniMultiAudio {
            Self::BelaMiniMultiAudio
        } else if raw == BelaHw_BelaHw_BelaMiniMultiTdm {
            Self::BelaMiniMultiTdm
        } else if raw == BelaHw_BelaHw_BelaMultiTdm {
            Self::BelaMultiTdm
        } else if raw == BelaHw_BelaHw_BelaMiniMultiI2s {
            Self::BelaMiniMultiI2s
        } else if raw == BelaHw_BelaHw_BelaEs9080 {
            Self::BelaEs9080
        } else if raw == BelaHw_BelaHw_BelaRevC {
            Self::BelaRevC
        } else if raw == BelaHw_BelaHw_Batch {
            Self::Batch
        } else {
            Self::Unrecognised { raw }
        }
    }

    /// The `BelaHw` this board is.
    ///
    /// An [`Unrecognised`](Self::Unrecognised) board gives back the
    /// number it was built from, so a value that came out of libbela
    /// can go back into it unchanged.
    #[must_use]
    pub const fn to_raw(self) -> BelaHw {
        match self {
            Self::NoHardware => BelaHw_BelaHw_NoHw,
            Self::Bela => BelaHw_BelaHw_Bela,
            Self::BelaMini => BelaHw_BelaHw_BelaMini,
            Self::GemStereo => BelaHw_BelaHw_GemStereo,
            Self::GemMulti => BelaHw_BelaHw_GemMulti,
            Self::Salt => BelaHw_BelaHw_Salt,
            Self::CtagFace => BelaHw_BelaHw_CtagFace,
            Self::CtagBeast => BelaHw_BelaHw_CtagBeast,
            Self::CtagFaceBela => BelaHw_BelaHw_CtagFaceBela,
            Self::CtagBeastBela => BelaHw_BelaHw_CtagBeastBela,
            Self::BelaMiniMultiAudio => BelaHw_BelaHw_BelaMiniMultiAudio,
            Self::BelaMiniMultiTdm => BelaHw_BelaHw_BelaMiniMultiTdm,
            Self::BelaMultiTdm => BelaHw_BelaHw_BelaMultiTdm,
            Self::BelaMiniMultiI2s => BelaHw_BelaHw_BelaMiniMultiI2s,
            Self::BelaEs9080 => BelaHw_BelaHw_BelaEs9080,
            Self::BelaRevC => BelaHw_BelaHw_BelaRevC,
            Self::Batch => BelaHw_BelaHw_Batch,
            Self::Unrecognised { raw } => raw,
        }
    }

    /// Whether this crate's headers name this board.
    ///
    /// False only for [`Unrecognised`](Self::Unrecognised), which is
    /// the whole of what it asks: a program that wants to refuse to run
    /// on hardware nobody measured it against starts here.
    #[must_use]
    pub const fn is_recognised(self) -> bool {
        !matches!(self, Self::Unrecognised { .. })
    }
}

impl fmt::Display for Board {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::NoHardware => f.write_str("NoHw"),
            Self::Bela => f.write_str("Bela"),
            Self::BelaMini => f.write_str("BelaMini"),
            Self::GemStereo => f.write_str("GemStereo"),
            Self::GemMulti => f.write_str("GemMulti"),
            Self::Salt => f.write_str("Salt"),
            Self::CtagFace => f.write_str("CtagFace"),
            Self::CtagBeast => f.write_str("CtagBeast"),
            Self::CtagFaceBela => f.write_str("CtagFaceBela"),
            Self::CtagBeastBela => f.write_str("CtagBeastBela"),
            Self::BelaMiniMultiAudio => f.write_str("BelaMiniMultiAudio"),
            Self::BelaMiniMultiTdm => f.write_str("BelaMiniMultiTdm"),
            Self::BelaMultiTdm => f.write_str("BelaMultiTdm"),
            Self::BelaMiniMultiI2s => f.write_str("BelaMiniMultiI2s"),
            Self::BelaEs9080 => f.write_str("BelaEs9080"),
            Self::BelaRevC => f.write_str("BelaRevC"),
            Self::Batch => f.write_str("Batch"),
            Self::Unrecognised { raw } => write!(f, "unrecognised({raw})"),
        }
    }
}

/// How [`Board::detect`] should go about finding out.
///
/// The modes are not interchangeable and none of them is a safe
/// default for every caller: they differ in which file they trust,
/// whether they fall back to scanning, and whether an answer of
/// [`Board::NoHardware`] means "nothing there" or "nobody wrote it
/// down". Two files are involved — `/run/bela/belaconfig`, which the
/// Bela daemon writes, and `~/.bela/belaconfig`, where a user overrides
/// it.
///
/// Not exhaustive: the modes are libbela's, and an image that adds one
/// adds it here rather than in this crate's idea of the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum DetectMode {
    /// Scan the peripherals and buses, and cache the result in
    /// `/run/bela/belaconfig`.
    ///
    /// The only mode that writes anything, and the only one whose cost
    /// is a bus transaction rather than a file read. It needs
    /// permission to write the cache, which a program not running as
    /// root may not have.
    Scan,
    /// Read `/run/bela/belaconfig`, and [`Scan`](Self::Scan) if it is
    /// not there.
    ///
    /// What a program wants when it just needs the answer: on a running
    /// board the daemon has already written the file, so this is a file
    /// read.
    Cache,
    /// Read `/run/bela/belaconfig`, and answer
    /// [`Board::NoHardware`] if it is not there.
    CacheOnly,
    /// Read `~/.bela/belaconfig`, and fall back to
    /// [`Cache`](Self::Cache) if it is not there.
    User,
    /// Read `~/.bela/belaconfig`, and answer [`Board::NoHardware`] if
    /// it is not there.
    ///
    /// The mode that asks what the user asked for, and nothing else. On
    /// a board with no such file — the measured one had none — it
    /// answers [`Board::NoHardware`] however plainly the hardware is
    /// there.
    UserOnly,
}

impl DetectMode {
    /// The `BelaHwDetectMode` this mode is.
    #[must_use]
    pub const fn to_raw(self) -> BelaHwDetectMode {
        match self {
            Self::Scan => BelaHwDetectMode_BelaHwDetectMode_Scan,
            Self::Cache => BelaHwDetectMode_BelaHwDetectMode_Cache,
            Self::CacheOnly => BelaHwDetectMode_BelaHwDetectMode_CacheOnly,
            Self::User => BelaHwDetectMode_BelaHwDetectMode_User,
            Self::UserOnly => BelaHwDetectMode_BelaHwDetectMode_UserOnly,
        }
    }

    /// Every mode, in the order the C enum declares them.
    ///
    /// For a program that reports what each one says rather than
    /// choosing between them — which is how the difference between them
    /// was measured. The order is the declaration's, not an order to
    /// ask them in: [`Scan`](Self::Scan) writes the file the three
    /// cache and user modes read, so a program calling all of them
    /// leaves it until last.
    ///
    /// A slice rather than an array, because the length is the C
    /// enum's: an image that adds a mode makes this longer, and a
    /// caller that had written down how many there were would be the
    /// only thing that broke.
    pub const ALL: &'static [Self] = &[
        Self::Scan,
        Self::Cache,
        Self::CacheOnly,
        Self::User,
        Self::UserOnly,
    ];
}

impl fmt::Display for DetectMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match *self {
            Self::Scan => "scan",
            Self::Cache => "cache",
            Self::CacheOnly => "cache-only",
            Self::User => "user",
            Self::UserOnly => "user-only",
        })
    }
}

/// A Bela version: major, minor and bugfix.
///
/// Ordered the way version numbers are — major first, then minor, then
/// bugfix — so a program can ask whether the library it found is new
/// enough for what it is about to use:
///
/// ```
/// use bela::Version;
///
/// assert!(Version::new(1, 18, 0) >= Version::new(1, 17, 0));
/// assert_eq!(Version::new(1, 18, 0).to_string(), "1.18.0");
/// ```
///
/// The three numbers are C `int`s in the API that fills them, and they
/// are kept as they arrived: deciding what a negative version number
/// means is not this crate's to do, and a saturated one would be a
/// number nobody reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// The major version.
    pub major: i32,
    /// The minor version.
    pub minor: i32,
    /// The bugfix version.
    pub bugfix: i32,
}

impl Version {
    /// What the headers this crate was built against said.
    ///
    /// `BELA_MAJOR_VERSION` and its siblings, as vendored in
    /// `bela-sys`. Compared with [`running`](Self::running) it says
    /// whether the board's image is the one the bindings describe;
    /// they differ when a binary is run on a board other than the one
    /// its sysroot came from.
    #[allow(
        clippy::cast_possible_wrap,
        reason = "bindgen types these macros as u32; they are small version numbers"
    )]
    pub const HEADERS: Self = Self {
        major: BELA_MAJOR_VERSION as i32,
        minor: BELA_MINOR_VERSION as i32,
        bugfix: BELA_BUGFIX_VERSION as i32,
    };

    /// A version from its three numbers.
    #[must_use]
    pub const fn new(major: i32, minor: i32, bugfix: i32) -> Self {
        Self {
            major,
            minor,
            bugfix,
        }
    }

    /// Asks the `libbela` this program is linked against which version
    /// it is.
    ///
    /// The library's own answer, not the headers': a binary built
    /// against one image and run on another reports the version it
    /// found there. No audio system is involved.
    ///
    /// Only available on the device target
    /// (`aarch64-unknown-linux-gnu`).
    #[cfg(bela_device)]
    #[must_use]
    pub fn running() -> Self {
        let mut major: c_int = 0;
        let mut minor: c_int = 0;
        let mut bugfix: c_int = 0;
        // Safety: three `int`s to write into, all live for the call and
        // written before it returns. libbela keeps no pointer to them.
        unsafe { bela_sys::Bela_getVersion(&raw mut major, &raw mut minor, &raw mut bugfix) };
        Self::new(major, minor, bugfix)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.bugfix)
    }
}

/// Keeps the C types the conversions are written against honest.
///
/// `BelaHw` is a signed `int` and `BelaHwDetectMode` an unsigned one,
/// which is what makes [`Board::Unrecognised`] carry a `c_int` and
/// [`DetectMode::to_raw`] produce a `c_uint`. A regenerated binding
/// that changed either would compile everywhere else and be wrong here.
const _: () = {
    assert!(
        size_of::<BelaHw>() == size_of::<c_int>(),
        "BelaHw is no longer the C int Board::Unrecognised carries"
    );
    assert!(
        size_of::<BelaHwDetectMode>() == size_of::<c_uint>(),
        "BelaHwDetectMode is no longer the C unsigned int DetectMode::to_raw produces"
    );
};

#[cfg(test)]
mod tests {
    use super::*;

    /// Every board the vendored headers name, with the constant it is.
    const NAMED: [(Board, BelaHw); 17] = [
        (Board::NoHardware, BelaHw_BelaHw_NoHw),
        (Board::Bela, BelaHw_BelaHw_Bela),
        (Board::BelaMini, BelaHw_BelaHw_BelaMini),
        (Board::GemStereo, BelaHw_BelaHw_GemStereo),
        (Board::GemMulti, BelaHw_BelaHw_GemMulti),
        (Board::Salt, BelaHw_BelaHw_Salt),
        (Board::CtagFace, BelaHw_BelaHw_CtagFace),
        (Board::CtagBeast, BelaHw_BelaHw_CtagBeast),
        (Board::CtagFaceBela, BelaHw_BelaHw_CtagFaceBela),
        (Board::CtagBeastBela, BelaHw_BelaHw_CtagBeastBela),
        (Board::BelaMiniMultiAudio, BelaHw_BelaHw_BelaMiniMultiAudio),
        (Board::BelaMiniMultiTdm, BelaHw_BelaHw_BelaMiniMultiTdm),
        (Board::BelaMultiTdm, BelaHw_BelaHw_BelaMultiTdm),
        (Board::BelaMiniMultiI2s, BelaHw_BelaHw_BelaMiniMultiI2s),
        (Board::BelaEs9080, BelaHw_BelaHw_BelaEs9080),
        (Board::BelaRevC, BelaHw_BelaHw_BelaRevC),
        (Board::Batch, BelaHw_BelaHw_Batch),
    ];

    #[test]
    fn every_named_board_reads_back_as_the_constant_it_is() {
        for (board, raw) in NAMED {
            assert_eq!(Board::from_raw(raw), board, "BelaHw {raw}");
            assert_eq!(board.to_raw(), raw, "{board}");
        }
    }

    #[test]
    fn a_board_the_headers_do_not_name_keeps_its_number() {
        // The values a later image would add: one past the end of the
        // list, and something far beyond it. Neither may be read as a
        // board this crate does know — that is the case that turns a
        // new image into a program acting on the wrong hardware.
        for raw in [BelaHw_BelaHw_Batch + 1, 99, c_int::MAX, c_int::MIN] {
            let board = Board::from_raw(raw);
            assert_eq!(board, Board::Unrecognised { raw });
            assert_eq!(
                board.to_raw(),
                raw,
                "the number must survive the round trip"
            );
            assert!(!board.is_recognised());
        }
    }

    #[test]
    fn no_hardware_is_a_detection_result_rather_than_an_unknown_board() {
        // -1 is a value the headers name, so it is not the unrecognised
        // case however negative it looks.
        assert_eq!(Board::from_raw(-1), Board::NoHardware);
        assert!(Board::NoHardware.is_recognised());
    }

    #[test]
    fn a_board_prints_the_name_libbela_gives_it() {
        // The spelling `/run/bela/belaconfig` uses, so that the two can
        // be compared without a translation table.
        assert_eq!(Board::GemStereo.to_string(), "GemStereo");
        assert_eq!(Board::NoHardware.to_string(), "NoHw");
        assert_eq!(
            Board::from_raw(17).to_string(),
            "unrecognised(17)",
            "a board with no name still says which number it is"
        );
    }

    #[test]
    fn every_detect_mode_is_the_constant_it_names() {
        let pairs = [
            (DetectMode::Scan, BelaHwDetectMode_BelaHwDetectMode_Scan),
            (DetectMode::Cache, BelaHwDetectMode_BelaHwDetectMode_Cache),
            (
                DetectMode::CacheOnly,
                BelaHwDetectMode_BelaHwDetectMode_CacheOnly,
            ),
            (DetectMode::User, BelaHwDetectMode_BelaHwDetectMode_User),
            (
                DetectMode::UserOnly,
                BelaHwDetectMode_BelaHwDetectMode_UserOnly,
            ),
        ];
        for (mode, raw) in pairs {
            assert_eq!(mode.to_raw(), raw, "{mode}");
        }
        // ALL is what a program reporting every mode iterates, so a
        // mode missing from it is a mode that never gets reported, and
        // it claims the C enum's order — which is the order the
        // constants themselves are in.
        let ordered: Vec<BelaHwDetectMode> = DetectMode::ALL.iter().map(|m| m.to_raw()).collect();
        assert_eq!(
            ordered,
            vec![0, 1, 2, 3, 4],
            "ALL is not in declaration order"
        );
        assert_eq!(DetectMode::ALL.len(), pairs.len());
        for (mode, _) in pairs {
            assert!(
                DetectMode::ALL.contains(&mode),
                "{mode} is missing from ALL"
            );
        }
    }

    #[test]
    fn versions_compare_by_major_then_minor_then_bugfix() {
        assert!(Version::new(1, 18, 0) > Version::new(1, 17, 9));
        assert!(Version::new(2, 0, 0) > Version::new(1, 99, 99));
        assert!(Version::new(1, 18, 1) > Version::new(1, 18, 0));
        assert_eq!(Version::new(1, 18, 0), Version::new(1, 18, 0));
    }

    #[test]
    fn a_version_prints_as_a_version() {
        assert_eq!(Version::new(1, 18, 0).to_string(), "1.18.0");
    }

    #[test]
    fn the_header_version_is_the_one_the_bindings_were_vendored_from() {
        // Written out rather than derived from the same macros the
        // constant is built from, which would pass whatever they said.
        // Re-vendoring the headers is meant to fail here: the version
        // is quoted in docs/board-facts.md, in the changelog and in
        // this module's documentation, and a run that has to come back
        // to this line is a run that visits those too.
        assert_eq!(
            Version::HEADERS,
            Version::new(1, 18, 0),
            "the vendored headers moved; update what quotes the version"
        );
    }
}
