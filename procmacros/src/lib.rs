//! Internal implementation details of `esp8266-hal`.
//!
//! Do not use this crate directly.

#![deny(warnings)]
#![allow(unused_braces)]

extern crate proc_macro;

use darling::FromMeta;
use proc_macro::TokenStream;
use quote::quote;
use std::collections::HashSet;
use std::iter;
use syn::{
    parse, parse::Parser, spanned::Spanned, AttrStyle, Attribute, FnArg, Ident, Item, ItemFn,
    ItemStatic, Meta, ReturnType, StaticMutability, Stmt, Type, Visibility,
};

// TODO:
// - check if all called function are in ram
// - check if all used data is in ram
// - check that no constants are use in the function (these cannot be forced to ram)
fn check_ram_function(_func: &syn::ItemFn) {
    //    eprintln!("{:?}", func);
}

#[derive(Debug, Default, FromMeta)]
#[darling(default)]
struct RamArgs {
    rtc: bool,
    uninitialized: bool,
    zeroed: bool,
}

/// This attribute allows placing statics, constants and functions into ram.
///
/// Options that can be specified are rtc to use the
/// RTC ram instead of the normal SRAM.
///
/// The uninitialized option will skip initialization of the memory
/// (e.g. to persist it across resets or deep sleep mode for the RTC RAM)
#[proc_macro_attribute]
pub fn ram(args: TokenStream, input: TokenStream) -> TokenStream {
    let args: proc_macro2::TokenStream = args.into();
    let attr_args = match darling::ast::NestedMeta::parse_meta_list(args) {
        Ok(v) => v,
        Err(e) => {
            return TokenStream::from(darling::Error::from(e).write_errors());
        }
    };

    let RamArgs {
        rtc,
        uninitialized,
        zeroed,
    } = match FromMeta::from_list(&attr_args) {
        Ok(v) => v,
        Err(e) => {
            return e.write_errors().into();
        }
    };

    if uninitialized && zeroed {
        return parse::Error::new(
            proc_macro2::Span::call_site(),
            "only one of `uninitialized` and `zeroed` may be specified",
        )
        .to_compile_error()
        .into();
    }

    let section_name_data = if rtc {
        if uninitialized {
            ".rtc.noinit"
        } else if zeroed {
            ".rtc.bss"
        } else {
            ".rtc.data"
        }
    } else if uninitialized {
        ".noinit"
    } else {
        ".data"
    };

    let section_name_text = if rtc { ".rtc.text" } else { ".rwtext" };

    let item: syn::Item = match syn::parse(input) {
        Ok(item) => item,
        Err(error) => return error.to_compile_error().into(),
    };

    let section = match &item {
        Item::Static(_) | Item::Const(_) => quote! {#[link_section=#section_name_data]},
        Item::Fn(ref function_item) => {
            if zeroed {
                return parse::Error::new(
                    function_item.span(),
                    "`zeroed` is not applicable to functions",
                )
                .to_compile_error()
                .into();
            }
            if uninitialized {
                return parse::Error::new(
                    function_item.span(),
                    "`uninitialized` is not applicable to functions",
                )
                .to_compile_error()
                .into();
            }
            check_ram_function(function_item);
            quote! {
                #[link_section=#section_name_text]
                #[inline(never)] // make certain function is not inlined
            }
        }
        _ => {
            return parse::Error::new(
                item.span(),
                "#[ram] can only be applied to functions, statics, and constants",
            )
            .to_compile_error()
            .into();
        }
    };

    let output = quote! {
        #section
        #item
    };
    output.into()
}

/// Marks a function as an interrupt handler
///
/// Used to handle on of the [interrupts](enum.Interrupt.html).
///
/// When specified between braces (`#[interrupt(example)]`) that interrupt will be used and the function
/// can have an arbitrary name. Otherwise the name of the function must be the name of the
/// interrupt.
#[proc_macro_attribute]
pub fn interrupt(args: TokenStream, input: TokenStream) -> TokenStream {
    let mut f: ItemFn = syn::parse(input).expect("`#[interrupt]` must be applied to a function");

    let args2: proc_macro2::TokenStream = args.into();
    let attr_args: Vec<Meta> = if args2.is_empty() {
        Vec::new()
    } else {
        match syn::punctuated::Punctuated::<Meta, syn::Token![,]>::parse_terminated
            .parse2(args2.clone())
        {
            Ok(p) => p.into_iter().collect(),
            Err(e) => return e.to_compile_error().into(),
        }
    };

    if attr_args.len() > 1 {
        return parse::Error::new(
            proc_macro2::Span::call_site(),
            "this attribute accepts at most one argument",
        )
        .to_compile_error()
        .into();
    }

    let ident = f.sig.ident.clone();
    let mut ident_s = &ident.clone();

    if attr_args.len() == 1 {
        match &attr_args[0] {
            Meta::Path(x) => {
                let Some(interrupt) = x.get_ident() else {
                    return parse::Error::new(x.span(), "interrupt name must be one identifier")
                        .to_compile_error()
                        .into();
                };
                ident_s = interrupt;
            }
            _ => {
                return parse::Error::new(
                    attr_args[0].span(),
                    "interrupt name must be one identifier",
                )
                .to_compile_error()
                .into();
            }
        }
    }

    // XXX should we blacklist other attributes?

    if let Err(error) = check_attr_whitelist(&f.attrs, WhiteListCaller::Interrupt) {
        return error;
    }

    let valid_signature = f.sig.constness.is_none()
        && f.vis == Visibility::Inherited
        && f.sig.abi.is_none()
        && f.sig.inputs.is_empty()
        && f.sig.generics.params.is_empty()
        && f.sig.generics.where_clause.is_none()
        && f.sig.variadic.is_none()
        && match f.sig.output {
            ReturnType::Default => true,
            ReturnType::Type(_, ref ty) => match **ty {
                Type::Tuple(ref tuple) => tuple.elems.is_empty(),
                Type::Never(..) => true,
                _ => false,
            },
        };

    if !valid_signature {
        return parse::Error::new(
            f.span(),
            "`#[interrupt]` handlers must have signature `[unsafe] fn() [-> !]`",
        )
        .to_compile_error()
        .into();
    }

    let (statics, stmts) = match extract_static_muts(f.block.stmts.iter().cloned()) {
        Err(e) => return e.to_compile_error().into(),
        Ok(x) => x,
    };

    f.sig.ident = Ident::new(
        &format!("__xtensa_lx_{}", f.sig.ident),
        proc_macro2::Span::call_site(),
    );
    f.sig.inputs.extend(statics.iter().map(|statik| {
        let ident = &statik.ident;
        let ty = &statik.ty;
        let attrs = &statik.attrs;
        syn::parse::<FnArg>(quote!(#[allow(non_snake_case)] #(#attrs)* #ident: &mut #ty).into())
            .unwrap()
    }));
    let ident_type = Ident::new(&ident_s.to_string().to_ascii_uppercase(), ident_s.span());
    f.block.stmts = iter::once(
        syn::parse2(quote! {{
            // Check that this interrupt actually exists
            ::esp8266_hal::interrupt::InterruptType::#ident_type;
        }})
        .unwrap(),
    )
    .chain(stmts)
    .collect();

    let tramp_ident = Ident::new(
        &format!("{}_trampoline", f.sig.ident),
        proc_macro2::Span::call_site(),
    );
    let ident = &f.sig.ident;

    let resource_args = statics
        .iter()
        .map(|statik| {
            let (ref cfgs, ref attrs) = extract_cfgs(statik.attrs.clone());
            let ident = &statik.ident;
            let ty = &statik.ty;
            let expr = &statik.expr;
            quote! {
                #(#cfgs)*
                {
                    #(#attrs)*
                    static mut #ident: #ty = #expr;
                    &mut #ident
                }
            }
        })
        .collect::<Vec<_>>();

    let (ref cfgs, ref attrs) = extract_cfgs(f.attrs.clone());

    let export_name = format!("__{}_interrupt", ident_s);

    quote!(
        #(#cfgs)*
        #(#attrs)*
        #[doc(hidden)]
        #[export_name = #export_name]
        pub unsafe extern "C" fn #tramp_ident() {
            #ident(
                #(#resource_args),*
            )
        }

        #[inline(always)]
        #f
    )
    .into()
}

/// Extracts `static mut` vars from the beginning of the given statements
fn extract_static_muts(
    stmts: impl IntoIterator<Item = Stmt>,
) -> Result<(Vec<ItemStatic>, Vec<Stmt>), parse::Error> {
    let mut istmts = stmts.into_iter();

    let mut seen = HashSet::new();
    let mut statics = vec![];
    let mut stmts = vec![];
    for stmt in istmts.by_ref() {
        match stmt {
            Stmt::Item(Item::Static(var)) => {
                if var.mutability != StaticMutability::None {
                    if seen.contains(&var.ident) {
                        return Err(parse::Error::new(
                            var.ident.span(),
                            format!("the name `{}` is defined multiple times", var.ident),
                        ));
                    }

                    seen.insert(var.ident.clone());
                    statics.push(var);
                } else {
                    stmts.push(Stmt::Item(Item::Static(var)));
                }
            }
            _ => {
                stmts.push(stmt);
                break;
            }
        }
    }

    stmts.extend(istmts);

    Ok((statics, stmts))
}

fn extract_cfgs(attrs: Vec<Attribute>) -> (Vec<Attribute>, Vec<Attribute>) {
    let mut cfgs = vec![];
    let mut not_cfgs = vec![];

    for attr in attrs {
        if eq(&attr, "cfg") {
            cfgs.push(attr);
        } else {
            not_cfgs.push(attr);
        }
    }

    (cfgs, not_cfgs)
}

enum WhiteListCaller {
    Interrupt,
}

fn check_attr_whitelist(attrs: &[Attribute], caller: WhiteListCaller) -> Result<(), TokenStream> {
    let whitelist = &[
        "doc",
        "link_section",
        "cfg",
        "allow",
        "warn",
        "deny",
        "forbid",
        "cold",
        "ram",
    ];

    'o: for attr in attrs {
        for val in whitelist {
            if eq(attr, val) {
                continue 'o;
            }
        }

        let err_str = match caller {
            WhiteListCaller::Interrupt => {
                "this attribute is not allowed on an interrupt handler controlled by esp8266_hal"
            }
        };

        return Err(parse::Error::new(attr.span(), err_str)
            .to_compile_error()
            .into());
    }

    Ok(())
}

/// Returns `true` if `attr.path` matches `name`
fn eq(attr: &Attribute, name: &str) -> bool {
    attr.style == AttrStyle::Outer && attr.path().is_ident(name)
}

/// Unified entry point attribute for ESP8266 applications.
///
/// # Sync (bare-metal) usage
///
/// Replaces `#[entry]` from `xtensa-lx-rt`. The function must return `-> !`:
///
/// ```rust,ignore
/// #[esp8266_hal::main]
/// fn main() -> ! {
///     loop { /* ... */ }
/// }
/// ```
///
/// # Async (WiFi / embassy) usage
///
/// Creates an embassy executor and spawns the user's async function as a task.
/// The optional `Spawner` parameter lets you spawn additional tasks:
///
/// ```rust,ignore
/// #[esp8266_hal::main]
/// async fn main(spawner: embassy_executor::Spawner) {
///     // your async application code
/// }
/// ```
#[proc_macro_attribute]
pub fn main(args: TokenStream, input: TokenStream) -> TokenStream {
    if !args.is_empty() {
        return parse::Error::new(
            proc_macro2::Span::call_site(),
            "`#[main]` attribute takes no arguments",
        )
        .to_compile_error()
        .into();
    }

    let f: ItemFn = match syn::parse(input) {
        Ok(f) => f,
        Err(e) => return e.to_compile_error().into(),
    };

    if f.sig.ident != "main" {
        return parse::Error::new(
            f.sig.ident.span(),
            "`#[main]` must be applied to a function named `main`",
        )
        .to_compile_error()
        .into();
    }

    if f.sig.asyncness.is_some() {
        main_async(f)
    } else {
        main_sync(f)
    }
}

/// Sync entry point — equivalent to `#[entry]` from xtensa-lx-rt.
///
/// Generates `#[export_name = "main"] pub unsafe extern "C" fn …() -> !`.
fn main_sync(f: ItemFn) -> TokenStream {
    // Must be `fn() -> !`
    let valid_signature = f.sig.constness.is_none()
        && f.sig.abi.is_none()
        && f.sig.inputs.is_empty()
        && f.sig.generics.params.is_empty()
        && f.sig.generics.where_clause.is_none()
        && f.sig.variadic.is_none()
        && match f.sig.output {
            ReturnType::Type(_, ref ty) => matches!(**ty, Type::Never(_)),
            _ => false,
        };

    if !valid_signature {
        return parse::Error::new(
            f.span(),
            "sync `#[main]` function must have signature `[unsafe] fn() -> !`",
        )
        .to_compile_error()
        .into();
    }

    let attrs = &f.attrs;
    let body = &f.block;
    let unsafety = &f.sig.unsafety;
    let user_main_call = if f.sig.unsafety.is_some() {
        quote! { unsafe { __esp8266_hal_user_main() } }
    } else {
        quote! { __esp8266_hal_user_main() }
    };

    #[cfg(feature = "wifi")]
    let output = quote! {
        #(#attrs)*
        #unsafety fn __esp8266_hal_user_main() -> ! {
            #body
        }

        #[export_name = "main"]
        pub extern "C" fn __esp8266_hal_main() -> ! {
            // Keep sync WiFi examples runtime-neutral, but still enter
            // through the SDK-style task/bootstrap context required by
            // the WiFi blob and compat scheduler.
            ::esp8266_hal::wifi::__init_runtime_heap();
            ::esp8266_hal::wifi::__init_newlib_reent();

            unsafe {
                ::esp8266_hal::wifi::__run_sdk_main_task(
                    __esp8266_hal_sync_main_task,
                )
            }
        }

        unsafe extern "C" fn __esp8266_hal_sync_main_task(_param: *mut ::core::ffi::c_void) {
            ::esp8266_hal::wifi::__reclaim_boot_stack_reservation();
            #user_main_call
        }
    };

    #[cfg(not(feature = "wifi"))]
    let output = quote! {
        #(#attrs)*
        #unsafety fn __esp8266_hal_user_main() -> ! {
            #body
        }

        #[export_name = "main"]
        pub extern "C" fn __esp8266_hal_main() -> ! {
            #user_main_call
        }
    };

    output.into()
}

/// Async entry point — embassy executor + task setup.
fn main_async(f: ItemFn) -> TokenStream {
    #[cfg(not(feature = "wifi"))]
    {
        parse::Error::new(
            f.sig.asyncness
                .expect("main_async is only called for async functions")
                .span(),
            "async `#[main]` requires the `wifi` feature; enable an esp8266-hal Wi-Fi feature or use a synchronous `#[main]`",
        )
        .to_compile_error()
        .into()
    }

    #[cfg(feature = "wifi")]
    {
        main_async_wifi(f)
    }
}

#[cfg(feature = "wifi")]
fn main_async_wifi(f: ItemFn) -> TokenStream {
    // Validate return type: must be () or -> !
    match &f.sig.output {
        ReturnType::Default => {} // () — fine
        ReturnType::Type(_, ty) => match **ty {
            Type::Never(..) => {} // -> ! — fine
            _ => {
                return parse::Error::new(
                    f.sig.output.span(),
                    "async `#[main]` function must return `()` or `-> !`",
                )
                .to_compile_error()
                .into();
            }
        },
    }

    // Validate parameter count: 0 or 1
    let has_spawner = match f.sig.inputs.len() {
        0 => false,
        1 => true,
        _ => {
            return parse::Error::new(
                f.sig.inputs.span(),
                "async `#[main]` function must take zero or one parameter (Spawner)",
            )
            .to_compile_error()
            .into();
        }
    };

    let body = &f.block;
    let attrs = &f.attrs;

    // Build the task function and entry point. The task always takes a Spawner
    // parameter (required by #[embassy_executor::task] when we pass it), and
    // the entry point always passes the spawner from executor.run().
    let (task_params, spawn_args) = if has_spawner {
        let param = &f.sig.inputs[0];
        (quote! { #param }, quote! { spawner })
    } else {
        (
            quote! { _spawner: ::embassy_executor::Spawner },
            quote! { spawner },
        )
    };

    let output = quote! {
        #[no_mangle]
        pub extern "C" fn main() -> ! {
            // Initialize the Rust heap (backed by esp-alloc).
            ::esp8266_hal::wifi::__init_runtime_heap();

            // Initialize newlib's _global_impure_ptr so C library functions
            // (calloc, printf, etc.) used by WiFi blob don't crash.
            ::esp8266_hal::wifi::__init_newlib_reent();

            unsafe { ::esp8266_hal::wifi::__run_sdk_main_task(__esp8266_hal_executor_task) }
        }

        unsafe extern "C" fn __esp8266_hal_executor_task(_param: *mut ::core::ffi::c_void) {
            ::esp8266_hal::wifi::__reclaim_boot_stack_reservation();
            static __EXECUTOR: ::esp8266_hal::wifi::__StaticCell<
                ::esp8266_hal::wifi::executor::Executor,
            > = ::esp8266_hal::wifi::__StaticCell::new();
            let executor = __EXECUTOR.init(::esp8266_hal::wifi::executor::Executor::new());
            executor.run(|spawner| {
                spawner.must_spawn(__esp8266_hal_bootstrap_task(spawner));
            })
        }

        #[::embassy_executor::task]
        async fn __esp8266_hal_bootstrap_task(spawner: ::embassy_executor::Spawner) {
            spawner.must_spawn(__esp8266_hal_main_task(#spawn_args));
        }

        #(#attrs)*
        #[::embassy_executor::task]
        async fn __esp8266_hal_main_task(#task_params) {
            #body
        }
    };

    output.into()
}
