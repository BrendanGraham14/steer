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
//        description: "Description string",
//        name: "name_string",
//        require_approval: true
//    }
//    async fn run(&self, p: ParamsStruct, cancel: Option<CancellationToken>) -> Result<String, ToolError> { ... }
// }
struct ToolDefinition {
    tool_name: Ident,
    params_struct: Path,
    description: LitStr,
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
        let mut description: Option<LitStr> = None;
        let mut name: Option<LitStr> = None;
        let mut require_approval: Option<LitBool> = None;

        for field in fields {
            let key_str = field.key.to_string();
            match key_str.as_str() {
                "params" => {
                    if params_struct.is_some() {
                        return Err(syn::Error::new_spanned(
                            field.key,
                            "Duplicate \'params\' field",
                        ));
                    }
                    if let syn::Expr::Path(expr_path) = field.value {
                        params_struct = Some(expr_path.path);
                    } else {
                        return Err(syn::Error::new_spanned(
                            field.value,
                            "Expected a path for \'params\' (e.g., crate::some::Params)",
                        ));
                    }
                }
                "description" => {
                    if description.is_some() {
                        return Err(syn::Error::new_spanned(
                            field.key,
                            "Duplicate \'description\' field",
                        ));
                    }
                    if let syn::Expr::Lit(ref expr_lit) = field.value {
                        if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                            description = Some(lit_str.clone());
                        } else {
                            return Err(syn::Error::new_spanned(
                                field.value,
                                "Expected a string literal for \'description\' (e.g., \"...\")",
                            ));
                        }
                    } else {
                        return Err(syn::Error::new_spanned(
                            field.value,
                            "Expected a string literal for \'description\' (e.g., \"...\")",
                        ));
                    }
                }
                "name" => {
                    if name.is_some() {
                        return Err(syn::Error::new_spanned(
                            field.key,
                            "Duplicate \'name\' field",
                        ));
                    }
                    if let syn::Expr::Lit(ref expr_lit) = field.value {
                        if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                            name = Some(lit_str.clone());
                        } else {
                            return Err(syn::Error::new_spanned(
                                field.value,
                                "Expected a string literal for \'name\' (e.g., \"tool_name\")",
                            ));
                        }
                    } else {
                        return Err(syn::Error::new_spanned(
                            field.value,
                            "Expected a string literal for \'name\' (e.g., \"tool_name\")",
                        ));
                    }
                }
                "require_approval" => {
                    if require_approval.is_some() {
                        return Err(syn::Error::new_spanned(
                            field.key,
                            "Duplicate \'require_approval\' field",
                        ));
                    }
                    if let syn::Expr::Lit(ref expr_lit) = field.value {
                        if let syn::Lit::Bool(lit_bool) = &expr_lit.lit {
                            require_approval = Some(lit_bool.clone());
                        } else {
                            return Err(syn::Error::new_spanned(
                                field.value,
                                "Expected a boolean literal for \'require_approval\' (e.g., true or false)",
                            ));
                        }
                    }
                }
                _ => {
                    return Err(syn::Error::new(
                        field.key.span(),
                        "Expected one of: 'params', 'description', 'name', 'require_approval'",
                    ));
                }
            }
        }

        // Check for missing fields
        let params_struct = params_struct
            .ok_or_else(|| syn::Error::new(input.span(), "Missing \'params\' field"))?;
        let description = description
            .ok_or_else(|| syn::Error::new(input.span(), "Missing \'description\' field"))?;
        let name = name.ok_or_else(|| syn::Error::new(input.span(), "Missing \'name\' field"))?;
        let require_approval = require_approval.unwrap_or(LitBool::new(true, input.span()));

        let run_function: syn::ItemFn = input.parse()?;

        if run_function.sig.ident != "run" {
            return Err(syn::Error::new_spanned(
                run_function.sig.ident,
                "Function must be named \'run\'",
            ));
        }

        Ok(ToolDefinition {
            tool_name,
            params_struct,
            description,
            name,
            require_approval,
            run_function,
        })
    }
}

#[proc_macro]
pub fn tool(input: TokenStream) -> TokenStream {
    let parsed_input = match syn::parse::<ToolDefinition>(input) {
        Ok(def) => def,
        Err(e) => return TokenStream::from(e.to_compile_error()),
    };

    let tool_struct_name = parsed_input.tool_name;
    let params_struct_name = parsed_input.params_struct;
    let description_str = parsed_input.description;
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

    let expanded = quote! {
        // Generate a constant for the tool name
        pub const #const_name: &str = #tool_name_literal;

        #[derive(Debug, Clone, Default)]
        pub struct #tool_struct_name;

        #run_function

        #[async_trait::async_trait]
        impl crate::tools::Tool for #tool_struct_name {
            fn name(&self) -> &'static str {
                #tool_name_str
            }

            fn description(&self) -> &'static str {
                #description_str
            }

            fn input_schema(&self) -> &'static crate::api::InputSchema {
                static SCHEMA: ::once_cell::sync::Lazy<crate::api::InputSchema> = ::once_cell::sync::Lazy::new(|| {
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

                    crate::api::InputSchema {
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
                token: Option<tokio_util::sync::CancellationToken>,
            ) -> std::result::Result<String, crate::tools::ToolError> {
                let params: #params_struct_name = ::serde_json::from_value(parameters.clone())
                    .map_err(|e| crate::tools::ToolError::InvalidParams(#tool_name_for_errors.to_string(), e.to_string()))?;

                if let Some(t) = &token {
                    if t.is_cancelled() {
                        return Err(crate::tools::ToolError::Cancelled(#tool_name_for_errors.to_string()));
                    }
                }

                run(self, params, token).await
            }

            fn requires_approval(&self) -> bool {
                #require_approval
            }
        }
    };

    TokenStream::from(expanded)
}
