//! Which types coerce to which, and by what: `parse_coerce.c` (REL_19_0 src/backend/parser).

use amphora_pg_catalog::data::{self, type_oid};
use amphora_pg_catalog::{Type, cast, type_by_oid};

/// `CoercionContext` (src/include/nodes/primnodes.h), in its order: a context allows the casts of
/// every context before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Context {
    Implicit,
    Assignment,
    Explicit,
}

/// `CoercionPathType` (src/include/parser/parse_coerce.h): how one type becomes another.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Path {
    /// No coercion.
    None,
    /// This function.
    Func(u32),
    /// No conversion: the values are binary-compatible.
    Relabel,
    /// The elements, one by one.
    Array,
    /// The source's output function, then the target's input function.
    ViaIo,
}

const TYPCATEGORY_STRING: u8 = b'S';

/// `getBaseType` (src/backend/utils/cache/lsyscache.c): a domain's base type, else the type itself.
pub fn base_type(oid: u32) -> u32 {
    let mut oid = oid;
    for _ in 0..data::TYPES.len() {
        match type_by_oid(oid) {
            Some(Type {
                kind: b'd', base, ..
            }) => oid = *base,
            _ => break,
        }
    }
    oid
}

/// `get_type_category_preferred`: a type's category and whether it is preferred there. A type the
/// catalog doesn't have has no category, which matches nothing.
fn category(oid: u32) -> (u8, bool) {
    type_by_oid(oid).map_or((0, false), |t| (t.category, t.preferred))
}

/// `TypeCategory` (parse_coerce.c:2981).
pub fn type_category(oid: u32) -> u8 {
    category(oid).0
}

/// `IsPreferredType` (parse_coerce.c:3001) for a type of a known category.
pub fn is_preferred(in_category: u8, oid: u32) -> bool {
    let (category, preferred) = category(oid);
    category == in_category && preferred
}

/// `get_element_type` (lsyscache.c): a true array's element type, else 0.
pub fn element_type(oid: u32) -> u32 {
    match type_by_oid(oid) {
        Some(t)
            if amphora_pg_catalog::proc_by_oid(t.subscript)
                .is_some_and(|p| p.src == "array_subscript_handler") =>
        {
            t.elem
        }
        _ => 0,
    }
}

fn kind(oid: u32) -> u8 {
    type_by_oid(oid).map_or(0, |t| t.kind)
}

/// `type_is_rowtype`'s composites (`ISCOMPLEX`, src/include/parser/parse_type.h): a type with a relation.
fn is_complex(oid: u32) -> bool {
    type_by_oid(base_type(oid)).is_some_and(|t| t.relid != 0)
}

fn is_complex_array(oid: u32) -> bool {
    let element = element_type(oid);
    element != 0 && is_complex(element)
}

fn range_subtype(oid: u32) -> u32 {
    data::PG_RANGE
        .iter()
        .find(|r| r.oid == oid)
        .map_or(0, |r| r.subtype)
}

/// `get_multirange_range`: a multirange's range type, else 0.
fn multirange_range(oid: u32) -> u32 {
    data::PG_RANGE
        .iter()
        .find(|r| r.multirange == oid)
        .map_or(0, |r| r.oid)
}

/// `IsPolymorphicType` (src/include/catalog/pg_type.h): both families.
pub fn is_polymorphic(oid: u32) -> bool {
    matches!(
        oid,
        type_oid::ANYELEMENT
            | type_oid::ANYARRAY
            | type_oid::ANYNONARRAY
            | type_oid::ANYENUM
            | type_oid::ANYRANGE
            | type_oid::ANYMULTIRANGE
            | type_oid::ANYCOMPATIBLE
            | type_oid::ANYCOMPATIBLEARRAY
            | type_oid::ANYCOMPATIBLENONARRAY
            | type_oid::ANYCOMPATIBLERANGE
            | type_oid::ANYCOMPATIBLEMULTIRANGE
    )
}

/// `find_coercion_pathway` (parse_coerce.c:3161). Its call on two arrays' element types is the
/// loop's second turn: an array's element is never itself an array, and two arrays are both of the
/// array category, so its I/O fallback never applies to them.
pub fn pathway(target: u32, source: u32, context: Context) -> Path {
    let (mut target, mut source) = (base_type(target), base_type(source));
    let mut elements = false;
    let path = loop {
        if target == source {
            break Path::Relabel;
        }
        if target == type_oid::INTERNAL || source == type_oid::INTERNAL {
            break Path::None;
        }
        if let Some(c) = cast(source, target) {
            let allowed = match c.context {
                b'i' => Context::Implicit,
                b'a' => Context::Assignment,
                _ => Context::Explicit,
            };
            break match c.method {
                _ if context < allowed => Path::None,
                b'f' => Path::Func(c.func),
                b'i' => Path::ViaIo,
                _ => Path::Relabel,
            };
        }
        let (to, from) = (element_type(target), element_type(source));
        if !matches!(target, type_oid::OIDVECTOR | type_oid::INT2VECTOR)
            && to != 0
            && from != 0
            && !elements
        {
            (target, source) = (base_type(to), base_type(from));
            elements = true;
            continue;
        }
        break if (context >= Context::Assignment && type_category(target) == TYPCATEGORY_STRING)
            || (context >= Context::Explicit && type_category(source) == TYPCATEGORY_STRING)
        {
            Path::ViaIo
        } else {
            Path::None
        };
    };
    match path {
        Path::None => Path::None,
        _ if elements => Path::Array,
        path => path,
    }
}

/// `find_typmod_coercion_function` (parse_coerce.c:3328): the function that fits a value of a type
/// to a type modifier, and whether it applies to an array's elements.
pub fn typmod_function(oid: u32) -> Path {
    let element = element_type(oid);
    let target = if element == 0 { oid } else { element };
    match cast(target, target) {
        Some(c) if c.func != 0 && element != 0 => Path::Array,
        Some(c) if c.func != 0 => Path::Func(c.func),
        _ => Path::None,
    }
}

/// `can_coerce_type` (parse_coerce.c:558). Inheritance and typed tables are not modelled: no
/// built-in type has them, and user row types have no OIDs yet (section 9).
pub fn can_coerce(inputs: &[u32], targets: &[u32], context: Context) -> bool {
    let mut generics = false;
    for (input, target) in inputs.iter().copied().zip(targets.iter().copied()) {
        if input == target {
            continue;
        }
        if input == type_oid::INTERNAL || target == type_oid::INTERNAL {
            return false;
        }
        if target == type_oid::ANY {
            continue;
        }
        if is_polymorphic(target) {
            generics = true;
            continue;
        }
        if input == type_oid::UNKNOWN
            || pathway(target, input, context) != Path::None
            || (input == type_oid::RECORD && is_complex(target))
            || (target == type_oid::RECORD && is_complex(input))
            || (target == data_record_array() && is_complex_array(input))
        {
            continue;
        }
        return false;
    }
    !generics || generics_consistent(inputs, targets)
}

/// `RECORDARRAYOID`: `record`'s array type.
fn data_record_array() -> u32 {
    type_by_oid(type_oid::RECORD).map_or(0, |t| t.array)
}

/// One polymorphic type's actual type: the first fixes it, the rest must agree.
fn agree(slot: &mut u32, actual: u32) -> bool {
    if *slot != 0 && *slot != actual {
        return false;
    }
    *slot = actual;
    true
}

/// `check_generic_type_consistency` (parse_coerce.c:1741).
fn generics_consistent(actual: &[u32], declared: &[u32]) -> bool {
    let (mut elem, mut array, mut range, mut multirange) = (0, 0, 0, 0);
    let (mut compatible_range, mut compatible_subtype, mut compatible_multirange) = (0, 0, 0);
    let mut compatible: Vec<u32> = Vec::new();
    let (mut nonarray, mut anyenum, mut compatible_nonarray) = (false, false, false);
    for (declared, actual) in declared.iter().copied().zip(actual.iter().copied()) {
        let flat = base_type(actual);
        match declared {
            type_oid::ANYNONARRAY => nonarray = true,
            type_oid::ANYENUM => anyenum = true,
            type_oid::ANYCOMPATIBLENONARRAY => compatible_nonarray = true,
            _ => {}
        }
        if !is_polymorphic(declared) || actual == type_oid::UNKNOWN {
            continue;
        }
        let consistent = match declared {
            type_oid::ANYELEMENT | type_oid::ANYNONARRAY | type_oid::ANYENUM => {
                agree(&mut elem, actual)
            }
            type_oid::ANYARRAY => agree(&mut array, flat),
            type_oid::ANYRANGE => agree(&mut range, flat),
            type_oid::ANYMULTIRANGE => agree(&mut multirange, flat),
            type_oid::ANYCOMPATIBLE | type_oid::ANYCOMPATIBLENONARRAY => {
                compatible.push(actual);
                true
            }
            type_oid::ANYCOMPATIBLEARRAY => {
                compatible.push(element_type(flat));
                element_type(flat) != 0
            }
            type_oid::ANYCOMPATIBLERANGE => {
                if compatible_range == 0 {
                    compatible_subtype = range_subtype(flat);
                    compatible.push(compatible_subtype);
                }
                agree(&mut compatible_range, flat) && compatible_subtype != 0
            }
            _ => agree(&mut compatible_multirange, flat) && multirange_range(flat) != 0,
        };
        if !consistent {
            return false;
        }
    }
    if array != 0 && array != type_oid::ANYARRAY {
        let element = element_type(array);
        if element == 0 || !agree(&mut elem, element) {
            return false;
        }
    }
    if multirange != 0 {
        let of = multirange_range(multirange);
        if of == 0 || !agree(&mut range, of) {
            return false;
        }
    }
    if range != 0 {
        let subtype = range_subtype(range);
        if subtype == 0 || !agree(&mut elem, subtype) {
            return false;
        }
    }
    if (nonarray && element_type(base_type(elem)) != 0) || (anyenum && kind(elem) != b'e') {
        return false;
    }
    if compatible_multirange != 0 {
        let of = multirange_range(compatible_multirange);
        if compatible_range == 0 {
            compatible_subtype = range_subtype(of);
            compatible.push(compatible_subtype);
        } else if of != compatible_range {
            return false;
        }
        if compatible_subtype == 0 {
            return false;
        }
    }
    let Some(common) = common_type(&compatible) else {
        return compatible.is_empty();
    };
    compatible.iter().all(|t| can_coerce_simple(*t, common))
        && !(compatible_nonarray && element_type(base_type(common)) != 0)
        && (compatible_subtype == 0 || compatible_subtype == common)
}

/// `can_coerce_type` on one pair whose target is no polymorphic type.
fn can_coerce_simple(input: u32, target: u32) -> bool {
    input == target
        || target == type_oid::ANY
        || input == type_oid::UNKNOWN
        || pathway(target, input, Context::Implicit) != Path::None
}

/// `select_common_type_from_oids` (parse_coerce.c:1482) with `noerror`: none when the categories differ.
fn common_type(types: &[u32]) -> Option<u32> {
    let first = *types.first()?;
    if first != type_oid::UNKNOWN && types.iter().all(|t| *t == first) {
        return Some(first);
    }
    let mut common = base_type(first);
    let (mut in_category, mut preferred) = category(common);
    for next in types.iter().map(|t| base_type(*t)) {
        if next == type_oid::UNKNOWN || next == common {
            continue;
        }
        let (next_category, next_preferred) = category(next);
        let take = if common == type_oid::UNKNOWN {
            true
        } else if next_category != in_category {
            return None;
        } else {
            !preferred && can_coerce_simple(common, next) && !can_coerce_simple(next, common)
        };
        if take {
            (common, in_category, preferred) = (next, next_category, next_preferred);
        }
    }
    Some(if common == type_oid::UNKNOWN {
        type_oid::TEXT
    } else {
        common
    })
}
