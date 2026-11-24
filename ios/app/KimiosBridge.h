// Bridging header: exposes the Rust library-OS entry point to Swift.
// The GUI is compiled as a no_std staticlib (libkimi_gui.a); its host syscall
// dispatcher is implemented in Swift via @_cdecl("kimi_host_syscall").
#ifndef KimiosBridge_h
#define KimiosBridge_h

#import <Foundation/Foundation.h>
#import "KimiISH.h"

// Runs the KimiOS GUI loop forever; call on a dedicated thread.
void kimi_gui_start(void) __attribute__((noreturn));

#endif
