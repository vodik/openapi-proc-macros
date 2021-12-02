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
}
