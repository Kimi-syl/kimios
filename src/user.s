// Tiny EL0 program. This blob is copied into a user page, mapped at
// 0x5000_0000, and entered with eret. Every address is PC-relative, so the
// copy is safe. Syscall ABI: x8 = number, x0..x2 = arguments, x0 = result.

.section .text
.global __user_prog_start
.global __user_prog_end

__user_prog_start:
    adr     x1, message
    mov     x2, #16             // message length
    mov     x0, #1              // fd = stdout
    mov     x8, #1              // SYS_WRITE
    svc     #0

    mov     x0, #0              // exit code
    mov     x8, #2              // SYS_EXIT
    svc     #0

1:
    b       1b

message:
    .ascii  "Hello from EL0!\n"
__user_prog_end:
