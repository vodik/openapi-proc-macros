# openapi-proc-macros

Work in progress experiment to see how viable it is to generate a rust client from an OpenAPI3 specification using proc macros:

```rust
mod petstore {
    api_gen_macro::openapi!("petstore-expanded.json");
}

#[tokio::main]
async fn main() {
    let client = petstore::Client::default();
    let pet = client
        .add_pet(&petstore::models::NewPet {
            name: "Paw".into(),
            tag: None,
        })
        .await
        .unwrap();

    dbg!(pet);
```
