contract;

use std::storage::storage_string::*;
use std::string::*;

abi HelloWorld {
    fn constructor();

    #[storage(read)]
    fn greet() -> StorageString;
}

storage {
    greet: StorageString = StorageString {},
    hello_world_constructor_called: bool = false,
}

impl HelloWorld for Contract {
    fn constructor() {
        require(!storage.hello_world_constructor_called.read(), "The HelloWorld constructor has already been called");
        storage.greet.write_slice(String::from_ascii_str("Hello World!"));
        storage.hello_world_constructor_called.write(true);
    }

    #[storage(read)]
    fn greet() -> StorageString {
        storage.greet.read()
    }
}
