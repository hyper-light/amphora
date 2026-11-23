use crate::alloc::TryClone;
use crate::function::{ExtFunc, ExternalFunc};
use crate::native_ext::{AggregateFunction, ExtensionState, FunctionArity, ScalarFunction};
use crate::sync::Arc;
use crate::types::{AggContext, ExternalAggState, IOResultOr};
use crate::{Connection, IOResult, OpenOptions, Register, Result, Value};
use amphora_engine_ext::ValueDestructor;

impl OpenOptions {
    pub fn extension_function(mut self, mut function: ExternalFunc) -> Self {
        function.name = crate::util::normalize_ident(&function.name);
        self.native_extensions.functions.push(Arc::new(function));
        self
    }
}

impl ExternalFunc {
    pub fn new_native_scalar<F: ScalarFunction + 'static>(
        name: String,
        arity: FunctionArity,
        deterministic: bool,
        function: F,
    ) -> Result<Self> {
        arity.validate()?;
        Ok(Self {
            name,
            func: ExtFunc::NativeScalar {
                arity,
                deterministic,
                function: Arc::new(function),
            },
        })
    }

    pub fn new_native_aggregate<F: AggregateFunction + 'static>(
        name: String,
        argc: i32,
        function: F,
    ) -> Result<Self> {
        Self::validate_arg_count(argc)?;
        Ok(Self {
            name,
            func: ExtFunc::NativeAggregate {
                argc,
                function: Arc::new(function),
            },
        })
    }
}

impl ExtFunc {
    pub(crate) fn call_scalar(
        &self,
        state: &mut ExtensionState,
        connection: &Arc<Connection>,
        args: &[Register],
    ) -> IOResultOr<Value> {
        match self {
            Self::NativeScalar { function, .. } => {
                crate::native_ext::step_scalar(state, function.as_ref(), connection, args)
            }
            Self::Scalar {
                context,
                callback,
                context_destructor,
                value_destructor,
                ..
            } => {
                let args: Vec<_> = args.iter().map(|arg| arg.get_value().to_ffi()).collect();
                let argv = if args.is_empty() {
                    std::ptr::null()
                } else {
                    args.as_ptr()
                };
                let result = unsafe {
                    callback(
                        *context,
                        args.len() as i32,
                        argv,
                        *context_destructor,
                        *value_destructor,
                    )
                };
                finish_call(result, *value_destructor, args)
                    .map(IOResult::Done)
                    .map_err(Into::into)
            }
            _ => unreachable!("aggregate called in scalar context"),
        }
    }

    pub(crate) fn step_aggregate(
        &self,
        registers: &mut [Register],
        acc_reg: usize,
        start_reg: usize,
    ) -> IOResultOr<()> {
        match self {
            Self::NativeAggregate { argc, function } => {
                let args = registers[start_reg..start_reg + (*argc).max(0) as usize]
                    .iter()
                    .map(|arg| arg.get_value().try_clone())
                    .collect::<Result<Vec<_>, _>>()?;
                crate::native_ext::step_aggregate(&mut registers[acc_reg], function.as_ref(), &args)
            }
            Self::Aggregate { step, .. } => {
                self.call_with_row(registers, acc_reg, start_reg, *step)?;
                Ok(IOResult::Done(()))
            }
            _ => unreachable!("scalar function called in aggregate context"),
        }
    }

    /// xInverse: takes back a row an earlier step added, for a window frame whose start moved.
    pub(crate) fn inverse_aggregate(
        &self,
        registers: &mut [Register],
        acc_reg: usize,
        start_reg: usize,
    ) -> Result<()> {
        let Self::Aggregate {
            window: Some(window),
            ..
        } = self
        else {
            return Err(crate::LimboError::InternalError(
                "AggInverse on an extension aggregate registered without xInverse".into(),
            ));
        };
        self.call_with_row(registers, acc_reg, start_reg, window.inverse)
    }

    /// Calls `f` (step or inverse) on the accumulator with the row's arguments. An error fails
    /// the statement at this row; the accumulator is released.
    fn call_with_row(
        &self,
        registers: &mut [Register],
        acc_reg: usize,
        start_reg: usize,
        f: amphora_engine_ext::StepFunction,
    ) -> Result<()> {
        let missing = || {
            crate::LimboError::InternalError(format!(
                "extension aggregate accumulator {acc_reg} past the registers"
            ))
        };
        let (context, state, argc, value_destructor) = {
            let aggregate = self.c_aggregate(registers.get_mut(acc_reg).ok_or_else(missing)?);
            (
                aggregate.context,
                aggregate.state,
                aggregate.argc,
                aggregate.value_destructor,
            )
        };
        let args: Vec<_> = registers
            .get(start_reg..start_reg.saturating_add(argc))
            .ok_or_else(|| {
                crate::LimboError::InternalError(format!(
                    "extension aggregate arguments {start_reg}..+{argc} past the registers"
                ))
            })?
            .iter()
            .map(|arg| arg.get_value().to_ffi())
            .collect();
        let argv = if args.is_empty() {
            std::ptr::null()
        } else {
            args.as_ptr()
        };
        let argc = i32::try_from(argc).map_err(|_| {
            crate::LimboError::InternalError(format!("{argc} extension aggregate arguments"))
        })?;
        let result = unsafe { f(context, state, argc, argv) };
        if let Err(err) = finish_call(result, value_destructor, args) {
            registers.get_mut(acc_reg).ok_or_else(missing)?.set_null();
            return Err(err);
        }
        Ok(())
    }

    pub(crate) fn finalize_aggregate(&self, accumulator: &mut Register) -> IOResultOr<Value> {
        match self {
            Self::NativeAggregate { function, .. } => {
                crate::native_ext::finalize_aggregate(accumulator, function.as_ref())
            }
            Self::Aggregate { .. } => {
                self.c_aggregate(accumulator);
                let Register::Aggregate(AggContext::External(mut aggregate)) =
                    std::mem::replace(accumulator, Register::Value(Value::Null))
                else {
                    return Err(crate::LimboError::InternalError(
                        "extension aggregate finalized without its accumulator".into(),
                    )
                    .into());
                };
                let Some(result) = aggregate.finish() else {
                    return Ok(IOResult::Done(Value::Null));
                };
                finish_call(result, aggregate.value_destructor, Vec::new())
                    .map(IOResult::Done)
                    .map_err(Into::into)
            }
            _ => unreachable!("scalar function called in aggregate context"),
        }
    }

    /// xValue: the aggregate's result so far, leaving the accumulator to go on.
    pub(crate) fn value_aggregate(&self, accumulator: &mut Register) -> Result<Value> {
        let Self::Aggregate {
            window: Some(window),
            ..
        } = self
        else {
            return Err(crate::LimboError::InternalError(
                "AggValue on an extension aggregate registered without xValue".into(),
            ));
        };
        let aggregate = self.c_aggregate(accumulator);
        let result = unsafe { (window.value)(aggregate.context, aggregate.state) };
        finish_call(result, aggregate.value_destructor, Vec::new())
    }

    fn c_aggregate<'a>(&self, accumulator: &'a mut Register) -> &'a ExternalAggState {
        if matches!(accumulator, Register::Value(Value::Null)) {
            let Self::Aggregate {
                context,
                init,
                step,
                finalize,
                argc,
                aggregate_destructor,
                value_destructor,
                ..
            } = self
            else {
                unreachable!("C aggregate requires C callbacks");
            };
            *accumulator = Register::Aggregate(AggContext::External(ExternalAggState {
                context: *context,
                state: unsafe { init(*context) },
                argc: (*argc).max(0) as usize,
                step_fn: *step,
                finalize_fn: *finalize,
                aggregate_destructor: *aggregate_destructor,
                value_destructor: *value_destructor,
            }));
        }
        let Register::Aggregate(AggContext::External(aggregate)) = accumulator else {
            unreachable!("C aggregate requires C accumulator state");
        };
        aggregate
    }
}

fn finish_call(
    mut result: amphora_engine_ext::Value,
    value_destructor: Option<ValueDestructor>,
    args: Vec<amphora_engine_ext::Value>,
) -> Result<Value> {
    let value = Value::from_ffi_ref(&result);
    if let Some(destructor) = value_destructor {
        unsafe { destructor(&mut result) };
    } else {
        unsafe { result.__free_internal_type() };
    }
    for arg in args {
        unsafe { arg.__free_internal_type() };
    }
    value
}
