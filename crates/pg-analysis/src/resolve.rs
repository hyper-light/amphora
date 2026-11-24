//! Which operator or function a name and its argument types mean: `parse_oper.c` and `parse_func.c`
//! (REL_19_0 src/backend/parser), over the candidates `namespace.c` (src/backend/catalog) collects.
//! The only operators and functions are the built-in ones: no CREATE OPERATOR or CREATE FUNCTION yet.

use amphora_pg_catalog::data::type_oid;
use amphora_pg_catalog::{
    Kind, Operator, Proc, SearchPath, operators_named, proc_by_oid, procs_named, type_named,
};

use crate::coerce::{Context, Path, base_type, can_coerce, is_preferred, pathway, type_category};
use crate::{Error, format_type, sqlstate};

/// `TYPCATEGORY_STRING` and the category no type has (src/include/catalog/pg_type.h).
const TYPCATEGORY_STRING: u8 = b'S';

/// A name as written: qualified by a schema, or not.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Name {
    pub schema: Option<String>,
    pub name: String,
}

impl Name {
    /// `NameListToString`.
    pub fn text(&self) -> String {
        match &self.schema {
            Some(schema) => format!("{schema}.{}", self.name),
            None => self.name.clone(),
        }
    }
}

/// `FuncDetailCode`'s refusals: no candidate takes the arguments, or several do and none is best.
enum Miss {
    NotFound,
    Multiple,
}

/// An operator or function that the name and argument count allow (`FuncCandidateList`).
struct Candidate {
    /// The operator's or function's OID; 0 for two functions that can't be told apart.
    oid: u32,
    args: Vec<u32>,
    position: usize,
    /// How many of the call's arguments a variadic parameter takes.
    variadic: usize,
    /// How many parameters take their defaults.
    defaults: usize,
}

/// `func_match_argtypes` (parse_func.c:1098): the candidates every argument coerces to implicitly.
fn matching(inputs: &[u32], candidates: Vec<Candidate>) -> Vec<Candidate> {
    candidates
        .into_iter()
        .filter(|c| can_coerce(inputs, &c.args, Context::Implicit))
        .collect()
}

/// The candidates with the most arguments that `matches`.
fn most(
    candidates: Vec<Candidate>,
    inputs: &[u32],
    matches: impl Fn(usize, u32, u32) -> bool,
) -> Vec<Candidate> {
    let count = |c: &Candidate| {
        inputs
            .iter()
            .zip(&c.args)
            .enumerate()
            .filter(|(i, (input, arg))| matches(*i, **input, **arg))
            .count()
    };
    let best = candidates.iter().map(count).max().unwrap_or(0);
    candidates
        .into_iter()
        .filter(|c| count(c) == best)
        .collect()
}

/// `func_select_candidate` (parse_func.c:1184): the best of several candidates, all of which the
/// arguments coerce to, none of which they match exactly.
fn select(inputs: &[u32], candidates: Vec<Candidate>) -> Option<Candidate> {
    let one = |c: Vec<Candidate>| -> Result<Candidate, Vec<Candidate>> {
        match <[Candidate; 1]>::try_from(c) {
            Ok([c]) => Ok(c),
            Err(c) => Err(c),
        }
    };
    let bases: Vec<u32> = inputs.iter().map(|t| base_type(*t)).collect();
    let unknown = |t: u32| t == type_oid::UNKNOWN;
    let unknowns = bases.iter().filter(|t| unknown(**t)).count();

    // Most exact matches (:1254).
    let candidates = most(candidates, &bases, |_, input, arg| {
        !unknown(input) && input == arg
    });
    let candidates = match one(candidates) {
        Ok(c) => return Some(c),
        Err(c) => c,
    };
    // Most exact matches or preferred types of the argument's category (:1296).
    let categories: Vec<u8> = bases.iter().map(|t| type_category(*t)).collect();
    let candidates = most(candidates, &bases, |i, input, arg| {
        !unknown(input)
            && (input == arg || categories.get(i).is_some_and(|c| is_preferred(*c, arg)))
    });
    let candidates = match one(candidates) {
        Ok(c) => return Some(c),
        Err(c) => c,
    };
    if unknowns == 0 {
        return None;
    }
    // The unknown arguments' categories (:1358): string if any candidate takes one, else the one
    // category all candidates agree on; then only candidates of that category, and of its
    // preferred type where any takes it.
    let mut slots: Vec<Option<(u8, bool)>> = Vec::new();
    let mut resolved = true;
    for (i, _) in bases.iter().enumerate().filter(|(_, t)| unknown(**t)) {
        let mut slot: Option<(u8, bool)> = None;
        let mut conflict = false;
        for arg in candidates.iter().filter_map(|c| c.args.get(i)) {
            let category = type_category(*arg);
            let preferred = is_preferred(category, *arg);
            slot = match slot {
                None => Some((category, preferred)),
                Some((c, p)) if c == category => Some((c, p || preferred)),
                Some(_) if category == TYPCATEGORY_STRING => Some((category, preferred)),
                Some(kept) => {
                    conflict = true;
                    Some(kept)
                }
            };
        }
        if conflict && slot.is_none_or(|(c, _)| c != TYPCATEGORY_STRING) {
            resolved = false;
            break;
        }
        slots.resize_with(i, || None);
        slots.push(slot);
    }
    let candidates = if resolved {
        let keeps = |c: &Candidate| {
            slots.iter().zip(&c.args).all(|(slot, arg)| match slot {
                None => true,
                Some((category, preferred)) => {
                    type_category(*arg) == *category
                        && (!preferred || is_preferred(*category, *arg))
                }
            })
        };
        let kept = candidates.iter().filter(|c| keeps(c)).count();
        let candidates: Vec<Candidate> = candidates
            .into_iter()
            .filter(|c| kept == 0 || keeps(c))
            .collect();
        match one(candidates) {
            Ok(c) => return Some(c),
            Err(c) => c,
        }
    } else {
        candidates
    };
    // Last gasp (:1508): the unknown arguments taken to be of the known ones' one type.
    let known = bases.iter().find(|t| !unknown(**t))?;
    if !bases.iter().all(|t| unknown(*t) || t == known) {
        return None;
    }
    let same = vec![*known; bases.len()];
    one(matching(&same, candidates)).ok()
}

/// `LookupExplicitNamespace` (namespace.c:3458) over the built-in operators and functions, which are
/// all pg_catalog's: the schemas a name may be found in, as their places on the path.
fn places(
    path: &SearchPath,
    name: &Name,
    namespace: impl Fn(&str) -> Option<u32>,
) -> Result<Vec<u32>, Error> {
    match &name.schema {
        None => Ok(path.namespaces(Kind::Function).collect()),
        Some(schema) => match namespace(schema) {
            Some(oid) => Ok(vec![oid]),
            None => Err(Error::new(
                sqlstate::INVALID_SCHEMA_NAME,
                format!("schema \"{schema}\" does not exist"),
            )),
        },
    }
}

/// `oper` (parse_oper.c:372) and, when `left` is none, `left_oper` (:517): the operator a name means
/// between operands of these types. 42883 when none does, 42725 when no one is best.
pub fn operator(
    path: &SearchPath,
    namespace: impl Fn(&str) -> Option<u32>,
    name: &Name,
    left: Option<u32>,
    right: u32,
) -> Result<&'static Operator, Error> {
    let kind = if left.is_some() { b'b' } else { b'l' };
    let places = places(path, name, namespace)?;
    let visible: Vec<(usize, &'static Operator)> = operators_named(&name.name)
        .iter()
        .filter(|o| o.kind == kind)
        .filter_map(|o| Some((places.iter().position(|ns| *ns == o.namespace)?, o)))
        .collect();
    let exactly = |left: u32, right: u32| {
        visible
            .iter()
            .filter(|(_, o)| o.left == left && o.right == right)
            .min_by_key(|(place, _)| *place)
            .map(|(_, o)| *o)
    };
    // binary_oper_exact (parse_oper.c:260): an unknown operand is taken to be of the other's type,
    // or of its base type.
    let exact = match left {
        None => exactly(0, right),
        Some(left) => {
            let unknown = type_oid::UNKNOWN;
            let (l, r, was_unknown) = match (left, right) {
                (l, r) if l == unknown => (r, r, true),
                (l, r) if r == unknown => (l, l, true),
                (l, r) => (l, r, false),
            };
            exactly(l, r).or_else(|| {
                let base = base_type(l);
                (was_unknown && base != l).then(|| exactly(base, base))?
            })
        }
    };
    if let Some(o) = exact {
        return Ok(o);
    }
    let inputs: Vec<u32> = left.into_iter().chain([right]).collect();
    // OpernameGetCandidates (namespace.c:1893): an operator earlier on the path hides one of the
    // same operand types.
    let candidates: Vec<Candidate> = visible
        .iter()
        .filter(|(place, o)| {
            !visible
                .iter()
                .any(|(p, v)| p < place && v.left == o.left && v.right == o.right)
        })
        .map(|(place, o)| Candidate {
            oid: o.oid,
            args: left.map(|_| o.left).into_iter().chain([o.right]).collect(),
            position: *place,
            variadic: 0,
            defaults: 0,
        })
        .collect();
    let found = oper_select_candidate(&inputs, candidates);
    let chosen = match found {
        Ok(oid) => visible.iter().find(|(_, o)| o.oid == oid).map(|(_, o)| *o),
        Err(Miss::NotFound) => None,
        Err(Miss::Multiple) => {
            return Err(Error::new(
                sqlstate::AMBIGUOUS_FUNCTION,
                format!(
                    "operator is not unique: {}",
                    operator_signature(name, left, right)
                ),
            ));
        }
    };
    chosen.ok_or_else(|| {
        Error::new(
            sqlstate::UNDEFINED_FUNCTION,
            format!(
                "operator does not exist: {}",
                operator_signature(name, left, right)
            ),
        )
    })
}

/// `oper_select_candidate` (parse_oper.c:304).
fn oper_select_candidate(inputs: &[u32], candidates: Vec<Candidate>) -> Result<u32, Miss> {
    let candidates = matching(inputs, candidates);
    match candidates.as_slice() {
        [] => Err(Miss::NotFound),
        [one] => Ok(one.oid),
        _ => select(inputs, candidates)
            .map(|c| c.oid)
            .ok_or(Miss::Multiple),
    }
}

/// `op_signature_string` (parse_oper.c:605).
fn operator_signature(name: &Name, left: Option<u32>, right: u32) -> String {
    match left {
        Some(left) => format!(
            "{} {} {}",
            format_type(left),
            name.text(),
            format_type(right)
        ),
        None => format!("{} {}", name.text(), format_type(right)),
    }
}

/// What a call means (`FuncDetailCode`'s normal and coercion answers).
#[derive(Clone, Debug)]
pub enum Call {
    /// This function, with the types its arguments take, how many of them a variadic parameter
    /// takes, and how many parameters take their defaults.
    Function {
        function: &'static Proc,
        args: Vec<u32>,
        variadic: usize,
        defaults: usize,
    },
    /// A cast to this type, written as a call of the type's name.
    Coercion(u32),
}

/// `func_get_detail` (parse_func.c:1577) for positional arguments: what a call of `name` means with
/// arguments of these types, `literal` where the first is a constant. 42883 when no function takes
/// them, 42725 when no one is best.
pub fn function(
    path: &SearchPath,
    namespace: impl Fn(&str) -> Option<u32>,
    name: &Name,
    inputs: &[u32],
    literal: bool,
) -> Result<Call, Error> {
    let places = places(path, name, &namespace)?;
    let candidates = function_candidates(&places, &name.name, inputs.len());
    let exact = candidates
        .iter()
        .position(|c| c.args.iter().take(inputs.len()).eq(inputs));
    let best = match exact {
        Some(i) => candidates.into_iter().nth(i),
        None => {
            if let [source] = inputs
                && let Some(target) = name_as_type(&places, &name.name)
                && coercion_call(target, *source, literal)
            {
                return Ok(Call::Coercion(target));
            }
            let candidates = matching(inputs, candidates);
            if candidates.len() > 1 {
                let Some(best) = select(inputs, candidates) else {
                    return Err(not_unique(name, inputs));
                };
                Some(best)
            } else {
                candidates.into_iter().next()
            }
        }
    };
    let Some(best) = best else {
        return Err(Error::new(
            sqlstate::UNDEFINED_FUNCTION,
            format!("function {} does not exist", signature(name, inputs)),
        ));
    };
    match proc_by_oid(best.oid) {
        Some(function) => Ok(Call::Function {
            function,
            args: best.args,
            variadic: best.variadic,
            defaults: best.defaults,
        }),
        None => Err(not_unique(name, inputs)),
    }
}

fn not_unique(name: &Name, inputs: &[u32]) -> Error {
    Error::new(
        sqlstate::AMBIGUOUS_FUNCTION,
        format!("function {} is not unique", signature(name, inputs)),
    )
}

/// `func_signature_string` (parse_func.c).
fn signature(name: &Name, inputs: &[u32]) -> String {
    let args: Vec<String> = inputs.iter().map(|t| format_type(*t)).collect();
    format!("{}({})", name.text(), args.join(", "))
}

/// `FuncNameAsType` (parse_func.c): the type a function's name names, unless it is a shell or composite.
fn name_as_type(places: &[u32], name: &str) -> Option<u32> {
    let t = places.iter().find_map(|ns| type_named(*ns, name))?;
    if t.relid == 0 { Some(t.oid) } else { None }
}

/// Whether a one-argument call of a type's name is a cast to it (parse_func.c:1700): always for an
/// unknown literal, else when the cast needs no function. The casts by function are found by their
/// own names. The source is a composite only for a string target, which the I/O path refuses here.
fn coercion_call(target: u32, source: u32, literal: bool) -> bool {
    if source == type_oid::UNKNOWN && literal {
        return true;
    }
    match pathway(target, source, Context::Explicit) {
        Path::Relabel => true,
        Path::ViaIo => {
            let complex = source == type_oid::RECORD
                || amphora_pg_catalog::type_by_oid(source).is_some_and(|t| t.relid != 0);
            !(complex && type_category(target) == TYPCATEGORY_STRING)
        }
        _ => false,
    }
}

/// `FuncnameGetCandidates` (namespace.c:1199) for positional arguments, variadic parameters
/// expanded and defaults allowed.
fn function_candidates(places: &[u32], name: &str, nargs: usize) -> Vec<Candidate> {
    let mut list: Vec<Candidate> = Vec::new();
    for p in procs_named(name) {
        let Some(position) = places.iter().position(|ns| *ns == p.namespace) else {
            continue;
        };
        let params = p.args.len();
        let variadic = params <= nargs && p.variadic != 0;
        let defaults = params.saturating_sub(nargs);
        if defaults > p.defaults || (params < nargs && !variadic) {
            continue;
        }
        let mut args = p.args.to_vec();
        let mut taken = 0;
        if variadic {
            args.truncate(params.saturating_sub(1));
            taken = nargs.saturating_sub(args.len());
            args.resize(nargs, p.variadic);
        }
        let new = Candidate {
            oid: p.oid,
            args,
            position,
            variadic: taken,
            defaults,
        };
        let written = |c: &Candidate| {
            let n = c.args.len().saturating_sub(c.defaults);
            c.args.iter().copied().take(n).collect::<Vec<u32>>()
        };
        let Some(old) = list.iter_mut().find(|c| written(c) == written(&new)) else {
            list.push(new);
            continue;
        };
        // namespace.c:1497: the earlier on the path, then the one that isn't variadic, else neither.
        let keep_old = old.position < new.position
            || (old.position == new.position && new.variadic > 0 && old.variadic == 0);
        let keep_new = old.position > new.position
            || (old.position == new.position && new.variadic == 0 && old.variadic > 0);
        if keep_new {
            *old = new;
        } else if !keep_old {
            old.oid = 0;
        }
    }
    list
}
