use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse_macro_input;
use syn::DeriveInput;

pub fn derive_agg_func(input: TokenStream) -> TokenStream {
    let ast = parse_macro_input!(input as DeriveInput);
    let struct_name = &ast.ident;

    let step_fn_name = format_ident!("{}_step", struct_name);
    let finalize_fn_name = format_ident!("{}_finalize", struct_name);
    let init_fn_name = format_ident!("{}_init", struct_name);
    let register_fn_name = format_ident!("register_{}", struct_name);
    let value_fn_name = format_ident!("{}_value", struct_name);
    let inverse_fn_name = format_ident!("{}_inverse", struct_name);
    let window = ast.attrs.iter().any(|attr| attr.path().is_ident("window"));

    let window_fns = quote! {
        #[cfg_attr(not(amphora_engine_core), no_mangle)]
        #[allow(non_snake_case)]
        pub extern "C" fn #value_fn_name(
            _context: usize,
            ctx: *mut ::amphora_engine_ext::AggCtx
        ) -> ::amphora_engine_ext::Value {
            unsafe {
                let ctx = &*ctx;
                let state = &*(ctx.state as *const <#struct_name as ::amphora_engine_ext::AggFunc>::State);
                match <#struct_name as ::amphora_engine_ext::WindowAggFunc>::value(state) {
                    Ok(val) => val,
                    Err(e) => ::amphora_engine_ext::Value::error_with_message(e.to_string()),
                }
            }
        }

        #[cfg_attr(not(amphora_engine_core), no_mangle)]
        #[allow(non_snake_case)]
        pub extern "C" fn #inverse_fn_name(
            _context: usize,
            ctx: *mut ::amphora_engine_ext::AggCtx,
            argc: i32,
            argv: *const ::amphora_engine_ext::Value,
        ) -> ::amphora_engine_ext::Value {
            unsafe {
                let ctx = &mut *ctx;
                let state = &mut *(ctx.state as *mut <#struct_name as ::amphora_engine_ext::AggFunc>::State);
                let args = match usize::try_from(argc) {
                    Ok(n) if n > 0 && !argv.is_null() => ::std::slice::from_raw_parts(argv, n),
                    _ => &[],
                };
                match <#struct_name as ::amphora_engine_ext::WindowAggFunc>::inverse(state, args) {
                    Ok(()) => ::amphora_engine_ext::Value::null(),
                    Err(e) => ::amphora_engine_ext::Value::error_with_message(e.to_string()),
                }
            }
        }
    };
    let window_fns = if window {
        window_fns
    } else {
        quote! {}
    };

    let register_call = if window {
        quote! {
            (api.register_window_function)(
                api.ctx,
                c_name.as_ptr(),
                #struct_name::ARGS,
                0,
                #struct_name::#init_fn_name
                    as ::amphora_engine_ext::InitAggFunction,
                #struct_name::#step_fn_name
                    as ::amphora_engine_ext::StepFunction,
                #struct_name::#finalize_fn_name
                    as ::amphora_engine_ext::FinalizeFunction,
                #struct_name::#value_fn_name
                    as ::amphora_engine_ext::ValueFunction,
                #struct_name::#inverse_fn_name
                    as ::amphora_engine_ext::StepFunction,
                None,
                None,
                None,
            )
        }
    } else {
        quote! {
            (api.register_aggregate_function)(
                api.ctx,
                c_name.as_ptr(),
                #struct_name::ARGS,
                0,
                #struct_name::#init_fn_name
                    as ::amphora_engine_ext::InitAggFunction,
                #struct_name::#step_fn_name
                    as ::amphora_engine_ext::StepFunction,
                #struct_name::#finalize_fn_name
                    as ::amphora_engine_ext::FinalizeFunction,
                None,
                None,
                None,
            )
        }
    };

    let expanded = quote! {
        impl #struct_name {
            #[cfg_attr(not(amphora_engine_core), no_mangle)]
            #[allow(non_snake_case)]
            pub extern "C" fn #init_fn_name(_context: usize) -> *mut ::amphora_engine_ext::AggCtx {
                let state = ::std::boxed::Box::new(<#struct_name as ::amphora_engine_ext::AggFunc>::State::default());
                let ctx = ::std::boxed::Box::new(::amphora_engine_ext::AggCtx {
                    state: ::std::boxed::Box::into_raw(state) as *mut ::std::os::raw::c_void,
                });
                ::std::boxed::Box::into_raw(ctx)
            }

            #[cfg_attr(not(amphora_engine_core), no_mangle)]
            #[allow(non_snake_case)]
            pub extern "C" fn #step_fn_name(
                _context: usize,
                ctx: *mut ::amphora_engine_ext::AggCtx,
                argc: i32,
                argv: *const ::amphora_engine_ext::Value,
            ) -> ::amphora_engine_ext::Value {
                unsafe {
                    let ctx = &mut *ctx;
                    let state = &mut *(ctx.state as *mut <#struct_name as ::amphora_engine_ext::AggFunc>::State);
                    let args = if argc <= 0 || argv.is_null() {
                        &[]
                    } else {
                        ::std::slice::from_raw_parts(argv, argc as usize)
                    };
                    match <#struct_name as ::amphora_engine_ext::AggFunc>::step(state, args) {
                        Ok(()) => ::amphora_engine_ext::Value::null(),
                        Err(e) => ::amphora_engine_ext::Value::error_with_message(e.to_string()),
                    }
                }
            }

            #window_fns

            #[cfg_attr(not(amphora_engine_core), no_mangle)]
            #[allow(non_snake_case)]
            pub extern "C" fn #finalize_fn_name(
                _context: usize,
                ctx: *mut ::amphora_engine_ext::AggCtx
            ) -> ::amphora_engine_ext::Value {
                unsafe {
                    let ctx = ::std::boxed::Box::from_raw(ctx);
                    let state = ::std::boxed::Box::from_raw(ctx.state as *mut <#struct_name as ::amphora_engine_ext::AggFunc>::State);
                    match <#struct_name as ::amphora_engine_ext::AggFunc>::finalize(*state) {
                        Ok(val) => val,
                        Err(e) => {
                            ::amphora_engine_ext::Value::error_with_message(e.to_string())
                        }
                    }
                }
            }

            #[cfg_attr(not(amphora_engine_core), no_mangle)]
            #[allow(non_snake_case)]
            pub unsafe extern "C" fn #register_fn_name(
                api: *const ::amphora_engine_ext::ExtensionApi
            ) -> ::amphora_engine_ext::ResultCode {
                if api.is_null() {
                    return ::amphora_engine_ext::ResultCode::Error;
                }

                let api = &*api;
                let name_str = #struct_name::NAME;
                let c_name = match ::std::ffi::CString::new(name_str) {
                    Ok(cname) => cname,
                    Err(_) => return ::amphora_engine_ext::ResultCode::Error,
                };

                #register_call
            }
        }
    };

    TokenStream::from(expanded)
}
