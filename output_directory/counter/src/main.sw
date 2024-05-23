contract;

abi Counter {
    #[storage(read)]
    fn count() -> u256;

    #[storage(read)]
    fn get() -> u256;

    #[storage(read, write)]
    fn inc();

    #[storage(read, write)]
    fn dec();
}

storage {
    count: u256 = 0,
}

impl Counter for Contract {
    #[storage(read)]
    fn count() -> u256 {
        storage.count.read()
    }

    #[storage(read)]
    fn get() -> u256 {
        storage.count.read()
    }

    #[storage(read, write)]
    fn inc() {
        storage.count.write(storage.count.read() + 1);
    }

    #[storage(read, write)]
    fn dec() {
        storage.count.write(storage.count.read() - 1);
    }
}
