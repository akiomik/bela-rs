/* Compile-time checks that the board's NE10 headers still describe
 * what bela-sys/src/ne10.rs declares.
 *
 * The declarations there are hand-written, so nothing regenerates when
 * a board image moves NE10: a changed typedef or parameter type would
 * link, run and corrupt memory. `cargo xtask check-vendor --board`
 * catches a changed header by diffing it against vendor/ne10; this
 * file catches the same drift at build time, needs no board, and runs
 * wherever the headers are — a cross build against BELA_SYSROOT and a
 * native build on the board alike. See ../build.rs.
 *
 * Every assertion is written in C primitives rather than in NE10's own
 * typedefs. `ne10_float32_t` on both sides of a comparison proves
 * nothing if the typedef is what moved, and what the Rust side names
 * is `f32`, `c_int` and "pointer to an opaque struct". So the typedefs
 * are pinned to primitives first, and everything after that is phrased
 * in those primitives.
 *
 * There is nothing to run: the file defines no symbol, and building it
 * at all is the check.
 */

#include <stddef.h>

#include <ne10/NE10_dsp.h>
#include <ne10/NE10_types.h>

/* The typedefs the Rust declarations assume. */
_Static_assert(__builtin_types_compatible_p(ne10_float32_t, float),
               "ne10_float32_t is no longer float; bela-sys declares f32");
_Static_assert(__builtin_types_compatible_p(ne10_int32_t, signed int),
               "ne10_int32_t is no longer int; bela-sys declares c_int");
_Static_assert(__builtin_types_compatible_p(ne10_fft_r2c_cfg_float32_t,
                                            ne10_fft_r2c_state_float32_t *),
               "the r2c plan handle is no longer a pointer to its state");

/* The one struct whose layout crosses the boundary. bela-sys declares
 * it #[repr(C)] with two f32 fields, and hands slices of it to the
 * transforms as they stand. */
_Static_assert(sizeof(ne10_fft_cpx_float32_t) == 8,
               "ne10_fft_cpx_float32_t is no longer two floats wide");
_Static_assert(_Alignof(ne10_fft_cpx_float32_t) == _Alignof(float),
               "ne10_fft_cpx_float32_t no longer has the alignment of float");
_Static_assert(offsetof(ne10_fft_cpx_float32_t, r) == 0,
               "the real part moved");
_Static_assert(offsetof(ne10_fft_cpx_float32_t, i) == 4,
               "the imaginary part moved");
_Static_assert(__builtin_types_compatible_p(
                   __typeof__(((ne10_fft_cpx_float32_t *)0)->r), float),
               "the real part is no longer a float");
_Static_assert(__builtin_types_compatible_p(
                   __typeof__(((ne10_fft_cpx_float32_t *)0)->i), float),
               "the imaginary part is no longer a float");

/* The four functions, by their whole type. A same-named symbol whose
 * parameters changed is the drift that survives everything above and
 * fails at the call. The plan pointer stays NE10's name because Rust
 * holds it opaque; every other position is the primitive Rust names. */
_Static_assert(__builtin_types_compatible_p(
                   __typeof__(&ne10_fft_alloc_r2c_float32),
                   ne10_fft_r2c_state_float32_t *(*)(signed int)),
               "ne10_fft_alloc_r2c_float32 changed signature");
_Static_assert(__builtin_types_compatible_p(
                   __typeof__(&ne10_fft_destroy_r2c_float32),
                   void (*)(ne10_fft_r2c_state_float32_t *)),
               "ne10_fft_destroy_r2c_float32 changed signature");
_Static_assert(__builtin_types_compatible_p(
                   __typeof__(&ne10_fft_r2c_1d_float32_neon),
                   void (*)(ne10_fft_cpx_float32_t *, float *,
                            ne10_fft_r2c_state_float32_t *)),
               "ne10_fft_r2c_1d_float32_neon changed signature");
_Static_assert(__builtin_types_compatible_p(
                   __typeof__(&ne10_fft_c2r_1d_float32_neon),
                   void (*)(float *, ne10_fft_cpx_float32_t *,
                            ne10_fft_r2c_state_float32_t *)),
               "ne10_fft_c2r_1d_float32_neon changed signature");
