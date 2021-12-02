use darling::ToTokens;
use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashSet},
    env,
    fs::File,
    io::BufReader,
    path,
    slice::SliceIndex,
};
use syn::{
    parse::{Parse, ParseStream},
    parse_macro_input,
};
use thiserror::Error;

#[derive(Debug, Deserialize, Serialize, Clone)]
struct Spec {
    openapi: String,
    //info: serde_json::Value,
    servers: Vec<Server>,
    paths: BTreeMap<String, Path>,
    components: Components,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct Server {
    url: String,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct Path {
    sumamry: Option<String>,
    description: Option<String>,
    get: Option<Operation>,
    put: Option<Operation>,
    post: Option<Operation>,
    delete: Option<Operation>,
    options: Option<Operation>,
    head: Option<Operation>,
    patch: Option<Operation>,
    trace: Option<Operation>,
}

impl Path {
    fn endpoints(self) -> impl Iterator<Item = (String, Operation)> {
        [
            ("get", self.get),
            ("put", self.put),
            ("post", self.post),
            ("delete", self.delete),
            ("options", self.options),
            ("head", self.head),
            ("patch", self.patch),
            ("trace", self.trace),
        ]
        .into_iter()
        .filter_map(|(method, operation)| operation.map(|operation| (method.into(), operation)))
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Operation {
    operation_id: String,
    description: Option<String>,
    parameters: Option<Vec<Parameter>>,
    request_body: Option<RequestBody>,
    responses: Option<BTreeMap<String, Response>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Parameter {}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct RequestBody {
    description: Option<String>,
    content: Option<BTreeMap<String, MediaType>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Response {
    description: String,
    content: Option<BTreeMap<String, MediaType>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct MediaType {
    schema: Schema,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct Components {
    schemas: BTreeMap<String, Schema>,
    securitySchemes: Option<BTreeMap<String, SecurityScheme>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct SecurityScheme {}

#[derive(Debug, Clone)]
enum TypedSecurityScheme {
    ApiKey,
    Http,
    OAuth,
    OpenIdConnect,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct Schema {
    #[serde(rename = "$ref")]
    r#ref: Option<String>,
    all_of: Option<Vec<Schema>>,
    r#type: Option<String>,
    format: Option<String>,
    required: Option<HashSet<String>>,
    items: Option<Box<Schema>>,
    properties: Option<BTreeMap<String, Property>>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
struct Property {
    r#type: String,
    format: Option<String>,
}

#[derive(Debug)]
struct OpenApi {
    specfile: syn::LitStr,
}

#[derive(Debug)]
enum Primative {
    Bool,
    I32,
    I64,
    F32,
    F64,
    String,
    Bytes,
    Binary,
    Date,
    DateTime,
    Password,
}

impl ToTokens for Primative {
    fn to_tokens(&self, tokens: &mut TokenStream) {
        tokens.extend(match self {
            Primative::Bool => quote! { bool },
            Primative::I32 => quote! { i32 },
            Primative::I64 => quote! { i64 },
            Primative::F32 => quote! { f32 },
            Primative::F64 => quote! { f64 },
            Primative::String => quote! { String },
            Primative::Bytes => quote! { String },
            Primative::Binary => quote! { String },
            Primative::Date => quote! { String },
            Primative::DateTime => quote! { String },
            Primative::Password => quote! { String },
        })
    }
}

#[derive(Debug)]
enum Type {
    Primative(Primative),
    Object,
    Array,
}

#[derive(Error, Debug)]
enum TypeError {
    #[error("invalid type {0}")]
    InvalidType(String),
    #[error("invalid format {format} for {typ}")]
    InvalidFormat { typ: String, format: String },
}

impl Type {
    fn parse(typ: &str, format: Option<&str>) -> Result<Self, TypeError> {
        match typ {
            "boolean" => Ok(Type::Primative(Primative::Bool)),
            "integer" => Ok(Type::Primative(match format {
                Some("int32") => Primative::I32,
                Some("int64") => Primative::I64,
                Some(format) => {
                    return Err(TypeError::InvalidFormat {
                        typ: typ.into(),
                        format: format.into(),
                    })
                }
                None => Primative::I64,
            })),
            "number" => Ok(Type::Primative(match format {
                Some("float") => Primative::F32,
                Some("double") => Primative::F64,
                Some(format) => {
                    return Err(TypeError::InvalidFormat {
                        typ: typ.into(),
                        format: format.into(),
                    })
                }
                None => Primative::F64,
            })),
            "string" => Ok(Type::Primative(match format {
                Some("bytes") => Primative::Bytes,
                Some("binary") => Primative::Binary,
                Some("date") => Primative::Date,
                Some("date-time") => Primative::DateTime,
                Some("password") => Primative::Password,
                Some(format) => {
                    return Err(TypeError::InvalidFormat {
                        typ: typ.into(),
                        format: format.into(),
                    })
                }
                None => Primative::String,
            })),
            "object" => Ok(Type::Object),
            "array" => Ok(Type::Array),
            typ => Err(TypeError::InvalidType(typ.into())),
        }
    }
}

#[derive(Debug)]
struct Field {
    ident: String,
    typ: Type,
    required: bool,
}

#[derive(Debug)]
struct ObjectSchema {
    fields: Vec<Field>,
}

#[derive(Debug)]
enum TypedSchema {
    Ref(String),
    Union(Vec<TypedSchema>),
    Primative(Primative),
    Object(ObjectSchema),
    Array(Box<TypedSchema>),
}

impl TryFrom<Schema> for TypedSchema {
    type Error = &'static str;

    fn try_from(schema: Schema) -> Result<Self, Self::Error> {
        if let Some(path) = schema.r#ref {
            Ok(TypedSchema::Ref(
                path.strip_prefix("#/components/schemas/").unwrap().into(),
            ))
        } else if let Some(union) = schema.all_of {
            let schema: Result<Vec<_>, _> =
                union.into_iter().map(|schema| schema.try_into()).collect();
            schema.map(TypedSchema::Union)
        } else if let Some(typ) = schema.r#type {
            match Type::parse(&typ, schema.format.as_deref()).unwrap() {
                Type::Primative(primative) => Ok(TypedSchema::Primative(primative)),
                Type::Object => {
                    let fields: Vec<_> =
                        schema
                            .properties
                            .map_or_else(Default::default, |properties| {
                                properties
                                    .iter()
                                    .map(|(name, property)| {
                                        let typ = Type::parse(
                                            &property.r#type,
                                            property.format.as_deref(),
                                        )
                                        .unwrap();

                                        let required = matches!(&schema.required, Some(required) if required.contains(name));

                                        Field {
                                            ident: name.into(),
                                            typ,
                                            required,
                                        }
                                    })
                                    .collect()
                            });

                    Ok(TypedSchema::Object(ObjectSchema { fields }))
                }
                Type::Array => {
                    let items = schema.items.as_deref().unwrap().clone().try_into().unwrap();
                    Ok(TypedSchema::Array(Box::new(items)))
                }
            }
        } else {
            Err("Unsupported schema definition")
        }
    }
}

impl Parse for OpenApi {
    fn parse(input: ParseStream) -> syn::parse::Result<Self> {
        let specfile = input.parse()?;
        Ok(OpenApi { specfile })
    }
}

fn to_snake_case(s: &str) -> String {
    let mut snake = String::new();
    for (i, ch) in s.char_indices() {
        if i > 0 && ch.is_uppercase() {
            snake.push('_');
        }
        snake.push(ch.to_ascii_lowercase());
    }
    snake
}

#[proc_macro]
pub fn openapi(input: proc_macro::TokenStream) -> proc_macro::TokenStream {
    let OpenApi { specfile } = parse_macro_input!(input as OpenApi);

    let specfile = specfile.value();
    let path: &path::Path = specfile.as_ref();

    let file = if path.is_relative() {
        File::open(path::Path::new(&env::var("CARGO_MANIFEST_DIR").unwrap()).join(path))
    } else {
        File::open(path)
    }
    .unwrap();

    let reader = BufReader::new(file);
    let spec: Spec = serde_json::from_reader(reader).unwrap();
    eprintln!("spec: {:#?}", spec);

    let servers = spec.servers.into_iter().map(|Server { url }| url);
    let server_count = servers.len();

    let operations = spec.paths.into_iter().flat_map(move |(url, paths)| {
        paths.endpoints().map(move |(method, operation)| {
            let ident = format_ident!("{}", to_snake_case(&operation.operation_id));

            let method = match method.as_str() {
                "get" => quote! { reqwest::Method::GET },
                "post" => quote! { reqwest::Method::POST },
                "delete" => quote! { reqwest::Method::DELETE },
                _ => todo!("method: {}", method),
            };

            let mut has_body = false;
            let mut has_ret = false;
            let mut args = vec![quote! { &self }];
            let mut ret = quote! { () };

            if let Some(body) = &operation.request_body {
                if let Some(content) = body
                    .content
                    .as_ref()
                    .and_then(|content| content.get("application/json").cloned())
                {
                    if let Ok(TypedSchema::Ref(component)) = content.schema.try_into() {
                        let ident = format_ident!("{}", component);
                        args.push(quote! { body: &models::#ident });
                        has_body = true;
                    }
                }
            }

            if let Some(responses) = &operation.responses {
                if let Some(response) = responses.get("200") {
                    if let Some(content) = response
                        .content
                        .as_ref()
                        .and_then(|content| content.get("application/json").cloned())
                    {
                        match content.schema.try_into() {
                            Ok(TypedSchema::Ref(component)) => {
                                let ident = format_ident!("{}", component);
                                ret = quote! { models::#ident };
                                has_ret = true;
                            }
                            Ok(TypedSchema::Array(inner)) => {
                                if let TypedSchema::Ref(component) = inner.as_ref() {
                                    let ident = format_ident!("{}", component);
                                    ret = quote! { Vec<models::#ident> };
                                    has_ret = true;
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }

            let request = if has_body {
                quote! {
                    let url = format!("{}{}", self.base_url, #url);
                    let resp = self.client.request(#method, url)
                        .json(body)
                        .send()
                        .await?;
                }
            } else {
                quote! {
                    let url = format!("{}{}", self.base_url, #url);
                    let resp = self.client.request(#method, url)
                        .send()
                        .await?;
                }
            };

            let response = if has_ret {
                quote! {
                    resp.json().await
                }
            } else {
                quote! {
                    Ok(())
                }
            };

            quote! {
                pub async fn #ident(#(#args),*) -> reqwest::Result<#ret> {
                    #request
                    #response
                }
            }
        })
    });

    let components: BTreeMap<String, TypedSchema> = spec
        .components
        .schemas
        .into_iter()
        .map(|(name, schema)| (name, schema.try_into().unwrap()))
        .collect();

    let components = gen_components(&components);

    let gen = quote! {
        const SERVERS: [&'static str; #server_count] = [#(#servers),*];

        pub mod models {
            #(#components)*
        }

        #[derive(Debug)]
        pub struct Client {
            client: reqwest::Client,
            base_url: reqwest::Url,
        }

        impl Default for Client {
            fn default() -> Self {
                Self {
                    client: reqwest::Client::new(),
                    base_url: reqwest::Url::parse(SERVERS[0]).unwrap(),
                }
            }
        }

        impl Client {
            pub fn new() -> Self {
                Self::default()
            }

            #(#operations)*
        }
    };

    eprintln!("{}", gen);
    gen.into()
}

fn gen_components(schemas: &BTreeMap<String, TypedSchema>) -> Vec<TokenStream> {
    let mut tokens = Vec::new();

    for (name, schema) in schemas {
        let name = name.rsplit('.').next().unwrap();
        let ident = format_ident!("{}", name);

        let gen = match schema {
            TypedSchema::Union(parts) => {
                let mut fields = Vec::new();

                for mut schema in parts {
                    if let TypedSchema::Ref(name) = schema {
                        schema = schemas.get(name).unwrap();
                    }

                    match schema {
                        TypedSchema::Object(object) => {
                            fields.extend(object.fields.iter().map(|field| {
                                let ident = format_ident!("{}", field.ident);
                                let typ = match &field.typ {
                                    Type::Primative(primative) => quote! { #primative },
                                    Type::Object => todo!("nested object"),
                                    Type::Array => todo!("nested array"),
                                };

                                if field.required {
                                    quote! { #ident: #typ }
                                } else {
                                    quote! { #ident: Option<#typ> }
                                }
                            }));
                        }
                        _ => todo!("unknown sub schema"),
                    }
                }

                quote! {
                    #[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
                    pub struct #ident {
                        #(pub #fields),*
                    }
                }
            }
            TypedSchema::Object(object) => {
                let fields = object.fields.iter().map(|field| {
                    let ident = format_ident!("{}", field.ident);
                    let typ = match &field.typ {
                        Type::Primative(primative) => quote! { #primative },
                        Type::Object => todo!("nested object"),
                        Type::Array => todo!("nested array"),
                    };

                    if field.required {
                        quote! { #ident: #typ }
                    } else {
                        quote! { #ident: Option<#typ> }
                    }
                });

                quote! {
                    #[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
                    pub struct #ident {
                        #(pub #fields),*
                    }
                }
            }
            _ => {
                quote! {
                    #[derive(Debug, serde::Deserialize, serde::Serialize, Clone)]
                    pub struct #ident {}
                }
            }
        };

        tokens.push(gen);
    }

    tokens
}
