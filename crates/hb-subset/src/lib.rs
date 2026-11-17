//! A small safe wrapper around the HarfBuzz subsetter (the one Google Fonts uses):
//! keeps the chosen characters (and every glyph their ligatures, alternates and
//! marks need) in TrueType, CFF and variable fonts.

use std::os::raw::{c_char, c_uint, c_void};

#[repr(C)]
struct Blob(c_void);
#[repr(C)]
struct Face(c_void);
#[repr(C)]
struct Input(c_void);
#[repr(C)]
struct Set(c_void);

const MEMORY_MODE_READONLY: c_uint = 1;
const SETS_UNICODE: c_uint = 1;
const SETS_NAME_ID: c_uint = 4;
const SETS_NAME_LANG_ID: c_uint = 5;
const SETS_LAYOUT_FEATURE_TAG: c_uint = 6;

const FLAGS_NO_HINTING: c_uint = 0x1;
const FLAGS_NOTDEF_OUTLINE: c_uint = 0x40;
const FLAGS_GLYPH_NAMES: c_uint = 0x80;

extern "C" {
    fn hb_blob_create(data: *const c_char, length: c_uint, mode: c_uint, user_data: *mut c_void, destroy: *mut c_void) -> *mut Blob;
    fn hb_blob_destroy(blob: *mut Blob);
    fn hb_blob_get_data(blob: *mut Blob, length: *mut c_uint) -> *const c_char;
    fn hb_face_create(blob: *mut Blob, index: c_uint) -> *mut Face;
    fn hb_face_destroy(face: *mut Face);
    fn hb_face_reference_blob(face: *mut Face) -> *mut Blob;
    fn hb_subset_input_create_or_fail() -> *mut Input;
    fn hb_subset_input_destroy(input: *mut Input);
    fn hb_subset_input_set(input: *mut Input, set_type: c_uint) -> *mut Set;
    fn hb_subset_input_set_flags(input: *mut Input, flags: c_uint);
    fn hb_subset_input_get_flags(input: *mut Input) -> c_uint;
    fn hb_subset_or_fail(source: *mut Face, input: *const Input) -> *mut Face;
    fn hb_set_add(set: *mut Set, codepoint: u32);
    fn hb_set_clear(set: *mut Set);
    fn hb_set_invert(set: *mut Set);
}

/// What to keep.
pub struct Options<'a> {
    /// The characters to keep; `None` keeps them all.
    pub unicodes: Option<&'a [u32]>,
    /// Drop TrueType hinting (smaller; screens at small sizes look a little softer).
    pub no_hinting: bool,
}

/// Subsets face `index` of `font` (TTF, OTF, or a collection) and returns a plain
/// TTF/OTF. Every OpenType feature, name record and glyph name is kept.
pub fn subset(font: &[u8], index: u32, opts: &Options) -> Option<Vec<u8>> {
    let len = c_uint::try_from(font.len()).ok()?;
    unsafe {
        let blob = hb_blob_create(font.as_ptr().cast(), len, MEMORY_MODE_READONLY, std::ptr::null_mut(), std::ptr::null_mut());
        let face = hb_face_create(blob, index);
        hb_blob_destroy(blob);
        let input = hb_subset_input_create_or_fail();
        if input.is_null() {
            hb_face_destroy(face);
            return None;
        }
        let unicodes = hb_subset_input_set(input, SETS_UNICODE);
        hb_set_clear(unicodes);
        match opts.unicodes {
            Some(list) => list.iter().for_each(|&u| hb_set_add(unicodes, u)),
            None => hb_set_invert(unicodes),
        }
        for kind in [SETS_NAME_ID, SETS_NAME_LANG_ID, SETS_LAYOUT_FEATURE_TAG] {
            let set = hb_subset_input_set(input, kind);
            hb_set_clear(set);
            hb_set_invert(set);
        }
        let mut flags = hb_subset_input_get_flags(input) | FLAGS_NOTDEF_OUTLINE | FLAGS_GLYPH_NAMES;
        if opts.no_hinting {
            flags |= FLAGS_NO_HINTING;
        }
        hb_subset_input_set_flags(input, flags);
        let out = hb_subset_or_fail(face, input);
        hb_subset_input_destroy(input);
        hb_face_destroy(face);
        if out.is_null() {
            return None;
        }
        let blob = hb_face_reference_blob(out);
        let mut n: c_uint = 0;
        let data = hb_blob_get_data(blob, &mut n);
        let bytes = if data.is_null() || n == 0 { None } else { Some(std::slice::from_raw_parts(data.cast::<u8>(), n as usize).to_vec()) };
        hb_blob_destroy(blob);
        hb_face_destroy(out);
        bytes
    }
}
