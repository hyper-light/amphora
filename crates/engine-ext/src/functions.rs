use crate::{ResultCode, Value};
use std::{
    ffi::{c_char, c_void},
    fmt::Display,
};

pub type ContextDestructor = unsafe extern "C" fn(context: usize);
pub type ValueDestructor = unsafe extern "C" fn(result: *mut Value);
pub type ScalarFunction = unsafe extern "C" fn(
    context: usize,
    argc: i32,
    argv: *const Value,
    context_destructor: Option<ContextDestructor>,
    value_destructor: Option<ValueDestructor>,
) -> Value;

pub type RegisterScalarFn = unsafe extern "C" fn(
    ctx: *mut c_void,
    name: *const c_char,
    argc: i32,
    deterministic: bool,
    context: usize,
    func: ScalarFunction,
    context_destructor: Option<ContextDestructor>,
    value_destructor: Option<ValueDestructor>,
) -> ResultCode;

pub type UnregisterFunctionFn =
    unsafe extern "C" fn(ctx: *mut c_void, name: *const c_char) -> ResultCode;

pub type RegisterAggFn = unsafe extern "C" fn(
    ctx: *mut c_void,
    name: *const c_char,
    args: i32,
    context: usize,
    init: InitAggFunction,
    step: StepFunction,
    finalize: FinalizeFunction,
    context_destructor: Option<ContextDestructor>,
    aggregate_destructor: Option<ContextDestructor>,
    value_destructor: Option<ValueDestructor>,
) -> ResultCode;

/// sqlite3_create_window_function's form of RegisterAggFn: an aggregate that also gives its
/// value without finishing (xValue) and takes a row back out (xInverse), so a window frame can
/// slide over it.
pub type RegisterWindowFn = unsafe extern "C" fn(
    ctx: *mut c_void,
    name: *const c_char,
    args: i32,
    context: usize,
    init: InitAggFunction,
    step: StepFunction,
    finalize: FinalizeFunction,
    value: ValueFunction,
    inverse: StepFunction,
    context_destructor: Option<ContextDestructor>,
    aggregate_destructor: Option<ContextDestructor>,
    value_destructor: Option<ValueDestructor>,
) -> ResultCode;

pub type InitAggFunction = unsafe extern "C" fn(context: usize) -> *mut AggCtx;
pub type StepFunction =
    unsafe extern "C" fn(context: usize, ctx: *mut AggCtx, argc: i32, argv: *const Value) -> Value;
pub type FinalizeFunction = unsafe extern "C" fn(context: usize, ctx: *mut AggCtx) -> Value;
/// The aggregate's value so far; unlike FinalizeFunction it leaves the state in place.
pub type ValueFunction = unsafe extern "C" fn(context: usize, ctx: *mut AggCtx) -> Value;

#[repr(C)]
pub struct AggCtx {
    pub state: *mut c_void,
}

pub trait AggFunc {
    type State: Default;
    type Error: Display;
    const NAME: &'static str;
    const ARGS: i32;

    /// An Err fails the statement at this row, as sqlite3_result_error in an xStep does.
    fn step(state: &mut Self::State, args: &[Value]) -> Result<(), Self::Error>;
    fn finalize(state: Self::State) -> Result<Value, Self::Error>;
}

/// An aggregate usable over any window frame (sqlite3_create_window_function): `value` reads the
/// result so far and `inverse` takes back a row an earlier `step` added. Derive with
/// `#[derive(AggregateDerive)] #[window]`.
pub trait WindowAggFunc: AggFunc {
    fn inverse(state: &mut Self::State, args: &[Value]) -> Result<(), Self::Error>;
    fn value(state: &Self::State) -> Result<Value, Self::Error>;
}

/// A scalar function that carries opaque, per-registration state.
///
/// `State` is constructed once per registration via [`ScalarFunc::init`], shared
/// by reference across every invocation, and dropped when the function is
/// unregistered or the owning connection is dropped. Because a single registration
/// is shared across connections and may be invoked concurrently, `State` must be
/// `Send + Sync`.
///
/// Stateless functions should use the `#[scalar]` attribute macro instead.
pub trait ScalarFunc {
    type State: Send + Sync;
    const NAME: &'static str;
    const ALIAS: Option<&'static str> = None;

    fn init() -> Self::State;
    fn call(state: &Self::State, args: &[Value]) -> Value;
}
