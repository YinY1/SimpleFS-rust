.PHONY: all clean

all: build copy_files

build:
	cargo build --release

copy_files:
	mkdir -p bin
	rsync -av --exclude='*.d' target/release/shell* target/release/simdisk* bin/

clean:
	cargo clean
	rm -rf bin