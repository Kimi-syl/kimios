#include <stdint.h>
#include <stdio.h>
uint64_t kimi_host_syscall(uint64_t n,uint64_t a0,uint64_t a1,uint64_t a2,uint64_t a3,uint64_t a4){(void)n;(void)a0;(void)a1;(void)a2;(void)a3;(void)a4;return 0;}
uint64_t kimi_host_cntpct(void){return 0;}
extern uint64_t kimi_test_osk(void);
int main(void){uint64_t r=kimi_test_osk();printf("osk self_test=%llu (63=all pass)\n",(unsigned long long)r);return r==63?0:1;}
