extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    Ident, ItemFn, LitBool, LitStr, Path, Token,
    parse::{Parse, ParseStream},
    punctuated::Punctuated,
};

// Define a struct to represent a single key-value pair within the braces
struct FieldValue {
    key: Ident,
    _colon: Token![:],
    value: syn::Expr, // Use syn::Expr for flexibility, handle specific types later
}

impl Parse for FieldValue {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(FieldValue {
            key: input.parse()?,
            _colon: input.parse()?,
            value: input.parse()?, // Parse as a general expression
        })
    }
}

// Structure to parse the macro input like:
// tool! {
//    ToolName {
//        params: ParamsStruct,
//        output: OutputType,
//        variant: ResultVariant,
//        description: "Description string",
//        name: "name_string",
//        require_approval: true
// }
//    async fn run(&self, p: ParamsStruct, cancel: Option<CancellationToken>) -> Result<OutputType, ToolError> { ... }
// }
#[allow(dead_code)]
struct ToolDefinition {
    tool_name: Ident,
    params_struct: Path,
    output_type: Path,
    variant: Ident,
    description: syn::Expr,
    require_approval: LitBool,
    name: LitStr,
    run_function: ItemFn,
}

impl Parse for ToolDefinition {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let tool_name: Ident = input.parse()?;
        let content;
        syn::braced!(content in input);

        // Parse the fields using Punctuated, providing the separator
        let fields: Punctuated<FieldValue, Token![,]> =
            content.parse_terminated(FieldValue::parse, Token![,])?;

        let mut params_struct: Option<Path> = None;
        let mut output_type: Option<Path> = None;
        let mut variant: Option<Ident> = None;
        let mut description: Option<syn::Expr> = None;
        let mut name: Option<LitStr> = None;
        let mut require_approval: Option<LitBool> = None;

        for field in fields {
            let key_str = field.key.to_string();
            match key_str.as_str() {
                "params" => {
                    if params_struct.is_some() {
                        return Err(syn::Error::new_spanned(
                            field.key,
                            "Duplicate 'params' field",
                        ));
                    }
                    if let syn::Expr::Path(expr_path) = field.value {
                        params_struct = Some(expr_path.path);
                    } else {
                        return Err(syn::Error::new_spanned(
                            field.value,
                            "Expected a path for 'params' (e.g., crate::some::Params)",
                        ));
                    }
                }
                "description" => {
                    if description.is_some() {
                        return Err(syn::Error::new_spanned(
                            field.key,
                            "Duplicate 'description' field",
                        ));
                    }
                    description = Some(field.value);
                }
                "name" => {
                    if name.is_some() {
                        return Err(syn::Error::new_spanned(
                            field.key,
                            "Duplicate 'name' field",
                        ));
                    }
                    if let syn::Expr::Lit(ref expr_lit) = field.value {
                        if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                            name = Some(lit_str.clone());
                        } else {
                            return Err(syn::Error::new_spanned(
                                field.value,
                                "Expected a string literal for 'name' (e.g., \"tool_name\")",
                            ));
                        }
                    } else {
                        return Err(syn::Error::new_spanned(
                            field.value,
                            "Expected a string literal for 'name' (e.g., \"tool_name\")",
                        ));
                    }
                }
                "output" => {
                    if output_type.is_some() {
                        return Err(syn::Error::new_spanned(
                            field.key,
                            "Duplicate 'output' field",
                        ));
                    }
                    if let syn::Expr::Path(expr_path) = field.value {
                        output_type = Some(expr_path.path);
                    } else {
                        return Err(syn::Error::new_spanned(
                            field.value,
                            "Expected a path for 'output' (e.g., MyOutputType)",
                        ));
                    }
                }
                "variant" => {
                    if variant.is_some() {
                        return Err(syn::Error::new_spanned(
                            field.key,
                            "Duplicate 'variant' field",
                        ));
                    }
                    if let syn::Expr::Path(ref expr_path) = field.value {
                        if let Some(ident) = expr_path.path.get_ident() {
                            variant = Some(ident.clone());
                        } else {
                            return Err(syn::Error::new_spanned(
                                field.value,
                                "Expected an identifier for 'variant' (e.g., Search)",
                            ));
                        }
                    } else {
                        return Err(syn::Error::new_spanned(
                            field.value,
                            "Expected an identifier for 'variant' (e.g., Search)",
                        ));
                    }
                }
                "require_approval" => {
                    if require_approval.is_some() {
                        return Err(syn::Error::new_spanned(
                            field.key,
                            "Duplicate 'require_approval' field",
                        ));
                    }
                    if let syn::Expr::Lit(ref expr_lit) = field.value {
                        if let syn::Lit::Bool(lit_bool) = &expr_lit.lit {
                            require_approval = Some(lit_bool.clone());
                        } else {
                            return Err(syn::Error::new_spanned(
                                field.value,
                                "Expected a boolean literal for 'require_approval' (e.g., true or false)",
                            ));
                        }
                    }
                }
                _ => {
                    return Err(syn::Error::new(
                        field.key.span(),
                        "Expected one of: 'params', 'output', 'variant', 'description', 'name', 'require_approval'",
                    ));
                }
            }
        }

        // Check for missing fields
        let params_struct = params_struct
            .ok_or_else(|| syn::Error::new(input.span(), "Missing 'params' field"))?;
        let output_type =
            output_type.ok_or_else(|| syn::Error::new(input.span(), "Missing 'output' field"))?;
        let variant = variant.ok_or_else(|| syn::Error::new(input.span(), "Missing 'variant' field"))?;
        let description =
            description.ok_or_else(|| syn::Error::new(input.span(), "Missing 'description' field"))?;
        let name = name.ok_or_else(|| syn::Error::new(input.span(), "Missing 'name' field"))?;
        let require_approval = require_approval.unwrap_or(LitBool::new(true, input.span()));

        let run_function: syn::ItemFn = input.parse()?;

        if run_function.sig.ident != "run" {
            return Err(syn::Error::new_spanned(
                run_function.sig.ident,
                "Function must be named 'run'",
            ));
        }

        Ok(ToolDefinition {
            tool_name,
            params_struct,
            output_type,
            variant,
            description,
            name,
            require_approval,
            run_function,
        })
    }
}

/// Tool macro that generates the implementation differently based on whether
/// it's used within conductor-tools or in an external crate.
///
/// When used in conductor-tools, imports will use `crate::`.
/// When used externally, imports will use `conductor_tools::`.
#[proc_macro]
pub fn tool(input: TokenStream) -> TokenStream {
    tool_impl(input, false)
}

/// Alternative version of the tool macro for use in external crates.
/// This ensures imports use `conductor_tools::` instead of `crate::`.
#[proc_macro]
pub fn tool_external(input: TokenStream) -> TokenStream {
    tool_impl(input, true)
}

fn tool_impl(input: TokenStream, is_external: bool) -> TokenStream {
    let parsed_input = match syn::parse::<ToolDefinition>(input) {
        Ok(def) => def,
        Err(e) => return TokenStream::from(e.to_compile_error()),
    };

    let tool_struct_name = parsed_input.tool_name;
    let params_struct_name = parsed_input.params_struct;
    let output_type = parsed_input.output_type;
    let variant = parsed_input.variant;
    let description_expr = parsed_input.description;
    let require_approval = parsed_input.require_approval;
    let run_function = parsed_input.run_function;
    let tool_name_literal = parsed_input.name;
    let tool_name_for_errors = tool_name_literal.value();
    let tool_name_str = quote! { #tool_name_literal };

    // Generate a constant name from the tool struct name
    let tool_struct_name_string = tool_struct_name.to_string();
    let const_name_string = {
        let mut result = String::new();
        for (i, c) in tool_struct_name_string.chars().enumerate() {
            if c.is_uppercase() && i != 0 {
                result.push('_');
            }
            result.push(c.to_ascii_uppercase());
        }
        result + "_NAME"
    };

    let const_name = syn::Ident::new(&const_name_string, tool_struct_name.span());

    // Choose import path based on whether we're in an external crate
    let (trait_path, context_path, error_path, schema_path, tool_result_path) = if is_external {
        (
            quote! { conductor_tools::Tool },
            quote! { conductor_tools::ExecutionContext },
            quote! { conductor_tools::ToolError },
            quote! { conductor_tools::InputSchema },
            quote! { conductor_tools::result::ToolResult },
        )
    } else {
        (
            quote! { crate::Tool },
            quote! { crate::ExecutionContext },
            quote! { crate::ToolError },
            quote! { crate::InputSchema },
            quote! { crate::result::ToolResult },
        )
    };

    // Check if output_type is a newtype wrapper (ends with "Result")
    let is_newtype = if let Some(last_segment) = output_type.segments.last() {
        let ident_str = last_segment.ident.to_string();
        ident_str == "GrepResult" || 
        ident_str == "AstGrepResult" || 
        ident_str == "MultiEditResult" || 
        ident_str == "ReplaceResult"
    } else {
        false
    };

    // Check if output_type is ExternalResult
    let is_external_result = if let Some(last_segment) = output_type.segments.last() {
        last_segment.ident.to_string() == "ExternalResult"
    } else {
        false
    };

    let from_impl = if is_external || is_external_result {
        // Skip generating From impls when used outside conductor-tools to avoid orphan rules
        // or when the output type is ExternalResult (which has a manual impl)
        quote! {}
    } else if is_newtype {
        quote! {
            impl From<#output_type> for #tool_result_path {
                fn from(r: #output_type) -> Self {
                    Self::#variant(r.0)
                }
            }
        }
    } else {
        quote! {
            impl From<#output_type> for #tool_result_path {
                fn from(r: #output_type) -> Self {
                    Self::#variant(r)
                }
            }
        }
    };


    let expanded = quote! {
        // Generate a constant for the tool name
        pub const #const_name: &str = #tool_name_literal;

        #[derive(Debug, Clone, Default)]
        pub struct #tool_struct_name;

        impl #tool_struct_name {
            pub fn name() -> &'static str {
                #tool_name_str
            }
        }

        #run_function

        #[async_trait::async_trait]
        impl #trait_path for #tool_struct_name {
            type Output = #output_type;

            fn name(&self) -> &'static str {
                #tool_struct_name::name()
            }

            fn description(&self) -> String {
                (#description_expr).into()
            }

            fn input_schema(&self) -> &'static #schema_path {
                static SCHEMA: ::once_cell::sync::Lazy<#schema_path> = ::once_cell::sync::Lazy::new(|| {
                    let settings = schemars::r#gen::SchemaSettings::draft07().with(|s| {
                        s.inline_subschemas = true;
                    });
                    let schema_gen = settings.into_generator();
                    // Use into_root_schema_for to get the full schema including definitions
                    let root_schema = schema_gen.into_root_schema_for::<#params_struct_name>();

                    // Extract properties and required fields directly from the schema object within the root schema
                    let (props, required) = if let Some(obj) = &root_schema.schema.object {
                        (
                            obj.properties.clone(),
                            obj.required.clone(),
                        )
                    } else {
                         (Default::default(), Default::default())
                    };

                    let properties: ::serde_json::Map<String, ::serde_json::Value> = props.into_iter().map(|(k, schema_obj)| {
                        // Convert the Schema object (which might be a reference) to JSON value
                        let val = ::serde_json::to_value(schema_obj).unwrap_or(::serde_json::Value::Null);
                         (k, val)
                    }).collect();

                    #schema_path {
                        properties: properties,
                        required: required.into_iter().collect(),
                        schema_type: "object".to_string(), // Assume top-level is always object for tool params
                    }
                });
                &SCHEMA
            }

            async fn execute(
                &self,
                parameters: ::serde_json::Value,
                context: &#context_path,
            ) -> std::result::Result<Self::Output, #error_path> {
                let params: #params_struct_name = ::serde_json::from_value(parameters.clone())
                    .map_err(|e| #error_path::invalid_params(#tool_name_for_errors, e.to_string()))?;

                if context.is_cancelled() {
                    return Err(#error_path::Cancelled(#tool_name_for_errors.to_string()));
                }

                run(self, params, context).await
            }

            fn requires_approval(&self) -> bool {
                #require_approval
            }
        }
        #from_impl
    };

    TokenStream::from(expanded)
}