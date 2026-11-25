#include <stdint.h>
#include <stdio.h>
uint64_t kimi_host_syscall(uint64_t n,uint64_t a0,uint64_t a1,uint64_t a2,uint64_t a3,uint64_t a4){(void)n;(void)a0;(void)a1;(void)a2;(void)a3;(void)a4;return 0;}
uint64_t kimi_host_cntpct(void){return 0;}
extern uint64_t kimi_test_pet(void);
int main(void){
  uint64_t r=kimi_test_pet();
  printf("pet self_test=0x%llx (0x1ff=all pass)\n",(unsigned long long)r);
  const char* names[]={"load","png-dims","pack-parse","frame-advance","oneshot-hold","oneshot-return","sleep","tap-reaction","render"};
  for(int i=0;i<9;i++) printf("  [%s] %s\n", names[i], (r>>i)&1?"PASS":"FAIL");
  return r==0x1ff?0:1;
}
