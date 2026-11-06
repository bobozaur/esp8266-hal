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

    if let Err(error) = check_attr_whitelist(&f.attrs) {
        return error;
    }

    if let Err(error) = validate_interrupt_signature(&f) {
        return error.to_compile_error().into();
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

    let export_name = interrupt_export_name(ident_s);

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
    attrs.into_iter().partition(|attr| eq(attr, "cfg"))
}

fn check_attr_whitelist(attrs: &[Attribute]) -> Result<(), TokenStream> {
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

        return Err(parse::Error::new(
            attr.span(),
            "this attribute is not allowed on an interrupt handler controlled by esp8266_hal",
        )
        .to_compile_error()
        .into());
    }

    Ok(())
}

fn validate_interrupt_signature(f: &ItemFn) -> syn::Result<()> {
    let valid_signature = f.sig.asyncness.is_none()
        && f.sig.constness.is_none()
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
        return Err(parse::Error::new(
            f.span(),
            "`#[interrupt]` handlers must have signature `[unsafe] fn() [-> !]`",
        ));
    }

    Ok(())
}

fn interrupt_export_name(interrupt: &Ident) -> String {
    format!("__{}_interrupt", interrupt.to_string().to_ascii_lowercase())
}

/// Returns `true` if `attr.path` matches `name`
fn eq(attr: &Attribute, name: &str) -> bool {
    attr.style == AttrStyle::Outer && attr.path().is_ident(name)
}

#[cfg(test)]
mod tests {

    #[test]
    fn interrupt_export_matches_dispatcher_for_all_accepted_case_spellings() {
        for name in ["uart", "UART", "Uart"] {
            let ident = syn::Ident::new(name, proc_macro2::Span::call_site());
            assert_eq!(super::interrupt_export_name(&ident), "__uart_interrupt");
        }
    }

    #[test]
    fn interrupt_rejects_futures_and_incompatible_signatures() {
        for declaration in [
            "async fn uart() {}",
            "const fn uart() {}",
            "extern \"C\" fn uart() {}",
            "fn uart<T>() {}",
            "fn uart() where (): Copy {}",
        ] {
            let function = syn::parse_str(declaration).unwrap();
            assert!(super::validate_interrupt_signature(&function).is_err());
        }
        let ordinary = syn::parse_quote! { unsafe fn uart() -> ! { loop {} } };
        assert!(super::validate_interrupt_signature(&ordinary).is_ok());
    }
}
