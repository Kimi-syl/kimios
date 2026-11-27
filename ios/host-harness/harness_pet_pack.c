#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
uint64_t kimi_host_syscall(uint64_t n,uint64_t a0,uint64_t a1,uint64_t a2,uint64_t a3,uint64_t a4){(void)n;(void)a0;(void)a1;(void)a2;(void)a3;(void)a4;return 0;}
uint64_t kimi_host_cntpct(void){return 0;}
extern uint64_t kimi_test_pet_pack(uint64_t,uint64_t,uint64_t,uint64_t);
static unsigned char* rd(const char*p,size_t*n){FILE*f=fopen(p,"rb");if(!f){perror(p);exit(2);}fseek(f,0,SEEK_END);*n=ftell(f);fseek(f,0,SEEK_SET);unsigned char*b=malloc(*n);if(fread(b,1,*n,f)!=*n){exit(2);}fclose(f);return b;}
int main(int argc,char**argv){
  if(argc<3){fprintf(stderr,"usage: %s pack.json atlas.png\n",argv[0]);return 2;}
  size_t jn,pn;unsigned char*j=rd(argv[1],&jn),*p=rd(argv[2],&pn);
  uint64_t r=kimi_test_pet_pack((uint64_t)(uintptr_t)j,jn,(uint64_t)(uintptr_t)p,pn);
  printf("import self_test=0x%llx (0xf=all pass: valid,dims,anims,render)\n",(unsigned long long)r);
  return r==0xf?0:1;
}
