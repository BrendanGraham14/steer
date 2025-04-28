extern crate proc_macro;

use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse::{Parse, ParseStream},
    Ident, ItemFn, LitStr, Path, Token,
};

// Structure to parse the macro input like:
// tool! {
//    ToolName {
//        params: ParamsStruct,
//        description: "Description string"
//    }
//    async fn run(&self, p: ParamsStruct, cancel: Option<CancellationToken>) -> Result<String, ToolError> { ... }
// }
struct ToolDefinition {
    tool_name: Ident,
    params_struct: Path,
    description: LitStr,
    run_function: ItemFn,
}

impl Parse for ToolDefinition {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let tool_name: Ident = input.parse()?;
        let content;
        syn::braced!(content in input);

        let mut params_struct: Option<Path> = None;
        let mut description: Option<LitStr> = None;

        while !content.is_empty() {
            let key: Ident = content.parse()?;
            content.parse::<Token![:]>()?;
            if key == "params" {
                params_struct = Some(content.parse()?);
            } else if key == "description" {
                description = Some(content.parse()?);
            } else {
                return Err(syn::Error::new(
                    key.span(),
                    "Expected 'params' or 'description'",
                ));
            }
            // Optional comma separator
            if content.peek(Token![,]) {
                content.parse::<Token![,]>()?;
            }
        }

        let params_struct =
            params_struct.ok_or_else(|| syn::Error::new(input.span(), "Missing 'params' field"))?;
        let description = description
            .ok_or_else(|| syn::Error::new(input.span(), "Missing 'description' field"))?;

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
            description,
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
    let tool_name_str = tool_struct_name.to_string();
    let run_function = parsed_input.run_function;

    let expanded = quote! {
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
                    let schema_gen = schemars::gen::SchemaGenerator::default();
                    // Use root_schema_for to get the full schema including definitions
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
                    .map_err(|e| crate::tools::ToolError::InvalidParams(#tool_name_str.to_string(), e.to_string()))?;

                if let Some(t) = &token {
                    if t.is_cancelled() {
                        return Err(crate::tools::ToolError::Cancelled(#tool_name_str.to_string()));
                    }
                }

                run(self, params, token).await
            }
        }
    };

    TokenStream::from(expanded)
}
