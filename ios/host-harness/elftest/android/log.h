#ifndef ANDROID_LOG_STUB
#define ANDROID_LOG_STUB
#include <stdio.h>
#define ANDROID_LOG_INFO 4
#define ANDROID_LOG_ERROR 6
#define __android_log_print(prio, tag, ...) fprintf(stderr, __VA_ARGS__)
#define __android_log_write(prio, tag, msg) fputs(msg, stderr)
#endif
