use proc_macro2::TokenStream;
use quote::{quote, ToTokens};
use syn::{
    spanned::Spanned, Attribute, Data, DataEnum, DeriveInput, Error, Fields, Generics, Ident,
};

use crate::utils::*;

pub fn expand_derive(ast: DeriveInput) -> Result<TokenStream, Error> {
    let StructAttributes { signature, .. } = StructAttributes::parse(&ast.attrs)?;

    let zv = zvariant_path();
    if let Some(signature) = signature {
        // Signature already provided, easy then!

        let name = ast.ident;
        let signature = match signature.as_str() {
            "dict" => "a{sv}".to_string(),
            _ => signature,
        };
        let parsed_signature = match signature.as_str() {
            "a{sv}" => quote! {
                #zv::parsed::Signature::static_dict(
                    &#zv::parsed::Signature::Str,
                    &#zv::parsed::Signature::Variant,
                )
            },
            // revert to parsing `signature` method's return.
            _ => quote! { #zv::parsed::Signature::from(#name::signature()) },
        };

        let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
        return Ok(quote! {
            impl #impl_generics #zv::Type for #name #ty_generics #where_clause {
                #[inline]
                fn signature() -> #zv::Signature<'static> {
                    // TODO: Move the parsed signature API into `zvariant_utils` and use it here to
                    // verify the signature string from user at compile time.
                    #zv::Signature::from_static_str(#signature).unwrap()
                }

                #[inline]
                fn parsed_signature() -> #zv::parsed::Signature {
                    #parsed_signature
                }
            }
        });
    }

    match ast.data {
        Data::Struct(ds) => match ds.fields {
            Fields::Named(_) if ds.fields.is_empty() => {
                impl_empty_struct(ast.ident, ast.generics, &zv)
            }
            Fields::Named(_) | Fields::Unnamed(_) => {
                impl_struct(ast.ident, ast.generics, ds.fields, &zv)
            }
            Fields::Unit => impl_unit_struct(ast.ident, ast.generics, &zv),
        },
        Data::Enum(data) => impl_enum(ast.ident, ast.generics, ast.attrs, data, &zv),
        _ => Err(Error::new(
            ast.span(),
            "only structs and enums supported at the moment",
        )),
    }
    .map(|implementation| {
        quote! {
            #[allow(deprecated)]
            #implementation
        }
    })
}

fn impl_struct(
    name: Ident,
    generics: Generics,
    fields: Fields,
    zv: &TokenStream,
) -> Result<TokenStream, Error> {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();
    let parsed_signature = signature_for_struct(&fields, zv, false);

    Ok(quote! {
        impl #impl_generics #zv::Type for #name #ty_generics #where_clause {
            #[inline]
            fn parsed_signature() -> #zv::parsed::Signature {
                #parsed_signature
            }
        }
    })
}

fn signature_for_struct(
    fields: &Fields,
    zv: &TokenStream,
    insert_enum_variant: bool,
) -> TokenStream {
    let field_types = fields.iter().map(|field| field.ty.to_token_stream());
    let new_type = match fields {
        Fields::Named(_) => false,
        Fields::Unnamed(_) if field_types.len() == 1 => true,
        Fields::Unnamed(_) => false,
        Fields::Unit => panic!("signature_for_struct must not be called for unit fields"),
    };
    let field_types_clone = field_types.clone();
    let parsed_signature = if new_type {
        quote! {#(
            <#field_types_clone as #zv::Type>::parsed_signature()
        ),*}
    } else {
        quote! {
            #zv::parsed::Signature::structure(
                [#(
                    <#field_types_clone as #zv::Type>::parsed_signature()
                ),*],
            )
        }
    };

    if insert_enum_variant {
        quote! {
            #zv::parsed::Signature::structure([
                <u32 as #zv::Type>::parsed_signature(),
                #parsed_signature,
            ])
        }
    } else {
        parsed_signature
    }
}

fn impl_unit_struct(
    name: Ident,
    generics: Generics,
    zv: &TokenStream,
) -> Result<TokenStream, Error> {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics #zv::Type for #name #ty_generics #where_clause {
            #[inline]
            fn parsed_signature() -> #zv::parsed::Signature {
                #zv::parsed::Signature::Unit
            }
        }
    })
}

fn impl_empty_struct(
    name: Ident,
    generics: Generics,
    zv: &TokenStream,
) -> Result<TokenStream, Error> {
    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics #zv::Type for #name #ty_generics #where_clause {
            #[inline]
            fn parsed_signature() -> #zv::parsed::Signature {
                #zv::parsed::Signature::U8
            }
        }
    })
}

fn impl_enum(
    name: Ident,
    generics: Generics,
    attrs: Vec<Attribute>,
    data: DataEnum,
    zv: &TokenStream,
) -> Result<TokenStream, Error> {
    let mut all_signatures: Vec<Result<TokenStream, Error>> = data
        .variants
        .iter()
        .map(|variant| signature_for_variant(variant, &attrs, zv))
        .collect();
    let parsed_signature = all_signatures.pop().unwrap()?;
    // Ensure all variants of the enum have the same number and type of fields.
    for sig in all_signatures {
        if sig?.to_string() != parsed_signature.to_string() {
            return Err(Error::new(
                name.span(),
                "all variants must have the same number and type of fields",
            ));
        }
    }

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    Ok(quote! {
        impl #impl_generics #zv::Type for #name #ty_generics #where_clause {
            #[inline]
            fn parsed_signature() -> #zv::parsed::Signature {
                #parsed_signature
            }
        }
    })
}

fn signature_for_variant(
    variant: &syn::Variant,
    attrs: &[Attribute],
    zv: &TokenStream,
) -> Result<TokenStream, Error> {
    let repr = attrs.iter().find(|attr| attr.path().is_ident("repr"));
    match &variant.fields {
        Fields::Unit => {
            let repr = match repr {
                Some(repr_attr) => repr_attr.parse_args()?,
                None => quote! { u32 },
            };

            Ok(quote! { <#repr as #zv::Type>::parsed_signature() })
        }
        Fields::Named(_) => Ok(signature_for_struct(&variant.fields, zv, true)),
        Fields::Unnamed(_) => Ok(signature_for_struct(&variant.fields, zv, true)),
    }
}
