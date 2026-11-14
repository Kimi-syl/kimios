#![no_std]
#![no_main]

use core::panic::PanicInfo;
use libkimi as k;

fn trim(input: &[u8]) -> &[u8] {
    let mut start = 0;
    let mut end = input.len();

    while start < end && (input[start] == b' ' || input[start] == b'\t') {
        start += 1;
    }
    while end > start
        && (input[end - 1] == b' ' || input[end - 1] == b'\t' || input[end - 1] == b'\r')
    {
        end -= 1;
    }

    &input[start..end]
}

fn split_word(input: &[u8]) -> (&[u8], &[u8]) {
    let mut index = 0;
    while index < input.len() && input[index] != b' ' && input[index] != b'\t' {
        index += 1;
    }
    let word = &input[..index];

    let mut rest = index;
    while rest < input.len() && (input[rest] == b' ' || input[rest] == b'\t') {
        rest += 1;
    }

    (word, &input[rest..])
}

/// Accept both `hello` and `/hello`.
fn with_slash<'a>(argument: &[u8], buffer: &'a mut [u8]) -> Option<&'a str> {
    if argument.is_empty() {
        return None;
    }

    let needs_slash = argument[0] != b'/';
    let length = argument.len() + needs_slash as usize;
    if length > buffer.len() {
        return None;
    }

    let mut offset = 0;
    if needs_slash {
        buffer[0] = b'/';
        offset = 1;
    }
    buffer[offset..offset + argument.len()].copy_from_slice(argument);

    core::str::from_utf8(&buffer[..length]).ok()
}

#[no_mangle]
pub extern "C" fn _start() -> ! {
    k::print("\nKimiOS user shell (libkimi)\n");
    k::print("Commands: ls, cat <file>, exec <file>, help, exit\n");

    let mut line = [0u8; 128];
    let mut path_buffer = [0u8; 96];
    let mut file_buffer = [0u8; 512];
    let mut listing = [0u8; 256];

    loop {
        k::print("$ ");
        let length = k::read_line(&mut line);
        let input = trim(&line[..length]);
        if input.is_empty() {
            continue;
        }

        let (command, rest) = split_word(input);
        let argument = trim(rest);

        if command == &b"help"[..] {
            k::print("  ls             list RamFS files\n");
            k::print("  cat <file>     print a file\n");
            k::print("  exec <file>    replace the shell with an ELF\n");
            k::print("  exit           halt the machine\n");
        } else if command == &b"ls"[..] {
            let count = k::list(&mut listing);
            for name in listing[..count].split(|&byte| byte == 0) {
                if !name.is_empty() {
                    k::print_bytes(name);
                    k::print("\n");
                }
            }
        } else if command == &b"cat"[..] {
            let Some(path) = with_slash(argument, &mut path_buffer) else {
                k::print("usage: cat <file>\n");
                continue;
            };

            let Some(handle) = k::open(path) else {
                k::print("cat: file not found\n");
                continue;
            };

            let mut offset = 0;
            loop {
                let count = k::read_file(handle, offset, &mut file_buffer);
                if count == 0 {
                    break;
                }
                k::print_bytes(&file_buffer[..count]);
                offset += count;
            }
            k::close(handle);
        } else if command == &b"exec"[..] {
            let Some(path) = with_slash(argument, &mut path_buffer) else {
                k::print("usage: exec <file>\n");
                continue;
            };

            k::print("exec: loading ");
            k::print(path);
            k::print("\n");
            k::exec(path);
            k::print("exec failed\n");
        } else if command == &b"exit"[..] {
            k::print("bye\n");
            k::exit(0);
        } else {
            k::print("unknown command: ");
            k::print_bytes(command);
            k::print("\n");
        }
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    k::print("shell panic\n");
    k::exit(1);
}
