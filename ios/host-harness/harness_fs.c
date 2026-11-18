// Native FS/net host dispatcher for the library-OS shell test.
// Implements the same syscall contract as the Swift KimiFS/KimiNet backends,
// backed by a temp directory (FS) and BSD sockets (net), to verify end-to-end
// that the unmodified libkimi shell works against the host backend.

#define _GNU_SOURCE
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <fcntl.h>
#include <dirent.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <sys/socket.h>
#include <netdb.h>
#include <arpa/inet.h>
#include <time.h>
#include <errno.h>

#define ERROR UINT64_MAX
static char diskRoot[256];
static uint64_t start_ns;

static uint64_t now_ns(void){struct timespec t;clock_gettime(CLOCK_MONOTONIC,&t);return (uint64_t)t.tv_sec*1000000000ull+t.tv_nsec;}
uint64_t kimi_host_cntpct(void){return now_ns()-start_ns;}

// ---- ramfs ----
static const char *ram_names[] = {"/readme","/version","/help"};
static const char *ram_data[] = {
    "KimiOS for iPad - library-OS edition.\nPersistent files live under /disk/.\n",
    "KimiOS 0.9.0-ios (library-OS, native ARM64, no QEMU)\n",
    "ls cat write rm mkdir rmdir mv stat df net dns fetch date time\n",
};
static int ram_index(const char *p){for(int i=0;i<3;i++)if(!strcmp(p,ram_names[i]))return i;return -1;}

// ---- handles ----
typedef struct { int used; int kind; char path[256]; } Handle; // kind 0 ram,1 disk
static Handle handles[64];
static int alloc_handle(int kind,const char *path){for(int i=0;i<64;i++)if(!handles[i].used){handles[i].used=1;handles[i].kind=kind;snprintf(handles[i].path,256,"%s",path);return i+100;}return -1;}

static int safe_rel(const char *rel){return strstr(rel,"..")==NULL;}
static void disk_path(const char *rel,char *out,size_t n){snprintf(out,n,"%s/%s",diskRoot,rel);}

static int is_dir(const char *p){struct stat st;return stat(p,&st)==0&&S_ISDIR(st.st_mode);}
static int file_exists(const char *p){struct stat st;return stat(p,&st)==0;}

static char *read_all(const char *p,size_t *len){
    int fd=open(p,O_RDONLY);if(fd<0)return NULL;
    struct stat st;fstat(fd,&st);size_t sz=st.st_size;
    char *b=malloc(sz+1);if(!b){close(fd);return NULL;}
    size_t got=read(fd,b,sz);close(fd);b[got]=0;*len=got;return b;
}

static uint64_t fs_open(const char *path){
    int ri=ram_index(path);
    if(ri>=0)return alloc_handle(0,path);
    if(strncmp(path,"/disk/",6))return ERROR;
    const char *rel=path+6;if(!safe_rel(rel))return ERROR;
    char p[512];disk_path(rel,p,sizeof p);
    if(!file_exists(p)||is_dir(p))return ERROR;
    return alloc_handle(1,rel);
}
static void mkdir_p(const char *dir){
    char tmp[512];snprintf(tmp,sizeof tmp,"%s",dir);
    size_t len=strlen(tmp);if(len&&tmp[len-1]=='/')tmp[len-1]=0;
    for(char *p=tmp+1;*p;p++){if(*p=='/'){*p=0;mkdir(tmp,0755);*p='/';}}
    mkdir(tmp,0755);
}
static void mkdirs_for(const char *filepath){
    char dir[512];snprintf(dir,sizeof dir,"%s",filepath);
    char *slash=strrchr(dir,'/');if(slash)*slash=0;
    mkdir_p(dir);
}
static uint64_t fs_create(const char *path){
    if(strncmp(path,"/disk/",6))return ERROR;
    const char *rel=path+6;if(!*rel||!safe_rel(rel))return ERROR;
    char p[512];disk_path(rel,p,sizeof p);
    mkdirs_for(p);
    int fd=open(p,O_WRONLY|O_CREAT|O_TRUNC,0644);if(fd<0)return ERROR;close(fd);
    return alloc_handle(1,rel);
}
static uint64_t fs_read(uint64_t h,uint64_t off,void *buf,uint64_t len){
    int idx=(int)h-100;if(idx<0||idx>=64||!handles[idx].used)return 0;
    if(handles[idx].kind==0){int ri=ram_index(handles[idx].path);if(ri<0)return 0;const char*d=ram_data[ri];size_t sz=strlen(d);if(off>=sz)return 0;uint64_t n=len<(sz-off)?len:(sz-off);memcpy(buf,d+off,n);return n;}
    char p[512];disk_path(handles[idx].path,p,sizeof p);
    int fd=open(p,O_RDONLY);if(fd<0)return 0;
    if(lseek(fd,(off_t)off,SEEK_SET)<0){close(fd);return 0;}
    ssize_t n=read(fd,buf,len);close(fd);return n<0?0:(uint64_t)n;
}
static uint64_t fs_write(uint64_t h,uint64_t off,const void *buf,uint64_t len){
    int idx=(int)h-100;if(idx<0||idx>=64||!handles[idx].used||handles[idx].kind!=1)return 0;
    char p[512];disk_path(handles[idx].path,p,sizeof p);
    struct stat st; size_t cursz=(stat(p,&st)==0)?(size_t)st.st_size:0;
    if(off==cursz){int fd=open(p,O_WRONLY|O_CREAT,0644);if(fd<0)return 0;lseek(fd,(off_t)off,SEEK_SET);ssize_t w=write(fd,buf,len);close(fd);return w==(ssize_t)len?len:0;}
    size_t sz=0;char *data=read_all(p,&sz);if(!data){data=malloc(1);sz=0;}
    size_t need=off+len;
    if(need>sz){data=realloc(data,need+1);memset(data+sz,0,need-sz);sz=need;}
    memcpy(data+off,buf,len);
    int fd=open(p,O_WRONLY|O_CREAT|O_TRUNC,0644);if(fd<0){free(data);return 0;}
    ssize_t w=write(fd,data,sz);close(fd);free(data);
    return w==(ssize_t)sz?len:0;
}
static uint64_t fs_close(uint64_t h){int idx=(int)h-100;if(idx<0||idx>=64||!handles[idx].used)return ERROR;handles[idx].used=0;return 0;}

static uint64_t push_str(void *buf,uint64_t cap,uint64_t *w,const char *s){
    uint64_t n=strlen(s);
    for(uint64_t i=0;i<n&&*w<cap;i++)((char*)buf)[(*w)++]=s[i];
    if(*w<cap)((char*)buf)[(*w)++]=0;
    return *w;
}
static uint64_t fs_list(void *buf,uint64_t cap){
    uint64_t w=0;
    for(int i=0;i<3;i++)push_str(buf,cap,&w,ram_names[i]);
    DIR *d=opendir(diskRoot);
    if(d){struct dirent *e;while((e=readdir(d))){if(e->d_name[0]=='.')continue;char p[512];disk_path(e->d_name,p,sizeof p);char nm[300];snprintf(nm,sizeof nm,"/disk/%s%s",e->d_name,is_dir(p)?"/":"");push_str(buf,cap,&w,nm);}closedir(d);}
    return w;
}
static uint64_t fs_listdir(const char *path,void *buf,uint64_t cap){
    const char *rel;
    if(!strcmp(path,"/disk"))rel="";
    else if(!strncmp(path,"/disk/",6))rel=path+6;
    else return ERROR;
    char p[512];disk_path(rel,p,sizeof p);
    if(!is_dir(p))return ERROR;
    uint64_t w=0;DIR *d=opendir(p);
    if(d){struct dirent *e;while((e=readdir(d))){if(e->d_name[0]=='.')continue;char cp[512];snprintf(cp,sizeof cp,"%s/%s",p,e->d_name);char entry[300];snprintf(entry,sizeof entry,"%s%s",e->d_name,is_dir(cp)?"/":"");push_str(buf,cap,&w,entry);}closedir(d);}
    return w;
}
static uint64_t fs_stat(const char *path,uint64_t *out){
    int ri=ram_index(path);
    if(ri>=0){out[0]=0;out[1]=strlen(ram_data[ri]);out[2]=0;return 0;}
    if(strncmp(path,"/disk/",6)&&strcmp(path,"/disk"))return ERROR;
    const char *rel=!strcmp(path,"/disk")?"":path+6;
    char p[512];disk_path(rel,p,sizeof p);
    struct stat st;if(stat(p,&st)!=0)return ERROR;
    if(S_ISDIR(st.st_mode)){out[0]=2;out[1]=0;out[2]=0;}
    else{out[0]=1;out[1]=st.st_size;out[2]=(st.st_size+511)/512;}
    return 0;
}
static uint64_t fs_unlink(const char *path){if(strncmp(path,"/disk/",6))return ERROR;const char*rel=path+6;if(!safe_rel(rel))return ERROR;char p[512];disk_path(rel,p,sizeof p);if(is_dir(p))return ERROR;return unlink(p)==0?0:ERROR;}
static uint64_t fs_mkdir(const char *path){const char*rel;if(!strcmp(path,"/disk"))rel="";else if(!strncmp(path,"/disk/",6))rel=path+6;else return ERROR;char p[512];disk_path(rel,p,sizeof p);if(is_dir(p))return 0;return mkdir(p,0755)==0?0:ERROR;}
static uint64_t fs_rmdir(const char *path){if(strncmp(path,"/disk/",6))return ERROR;const char*rel=path+6;char p[512];disk_path(rel,p,sizeof p);if(!is_dir(p))return ERROR;return rmdir(p)==0?0:ERROR;}
static uint64_t fs_rename(const char *o,const char *n){if(strncmp(o,"/disk/",6)||strncmp(n,"/disk/",6))return ERROR;char op[512],np[512];disk_path(o+6,op,sizeof op);disk_path(n+6,np,sizeof np);if(!file_exists(op)||file_exists(np))return ERROR;return rename(op,np)==0?0:ERROR;}

// ---- net ----
static int socks[16];static int nsock;
static uint64_t net_resolve(const char *name,void *out){
    struct addrinfo hints,*res;memset(&hints,0,sizeof hints);hints.ai_family=AF_INET;hints.ai_socktype=SOCK_STREAM;
    if(getaddrinfo(name,NULL,&hints,&res)!=0)return ERROR;
    uint64_t r=ERROR;
    for(struct addrinfo *a=res;a;a=a->ai_next){if(a->ai_family==AF_INET){struct sockaddr_in *sin=(struct sockaddr_in*)a->ai_addr;memcpy(out,&sin->sin_addr.s_addr,4);r=0;break;}}
    freeaddrinfo(res);return r;
}
static uint64_t net_connect(uint64_t ip,uint64_t port,uint64_t timeout){
    char ipstr[32];snprintf(ipstr,sizeof ipstr,"%llu.%llu.%llu.%llu",(unsigned long long)(ip>>24)&0xff,(unsigned long long)(ip>>16)&0xff,(unsigned long long)(ip>>8)&0xff,(unsigned long long)ip&0xff);
    int fd=socket(AF_INET,SOCK_STREAM,0);if(fd<0)return ERROR;
    struct sockaddr_in a;memset(&a,0,sizeof a);a.sin_family=AF_INET;a.sin_port=htons((uint16_t)port);a.sin_addr.s_addr=inet_addr(ipstr);
    (void)timeout;
    if(connect(fd,(struct sockaddr*)&a,sizeof a)!=0){close(fd);return ERROR;}
    if(nsock>=16){close(fd);return ERROR;}
    socks[nsock]=fd;return nsock++;
}
static uint64_t net_send(uint64_t slot,const void *buf,uint64_t len){if(slot>=(uint64_t)nsock)return 0;ssize_t n=send(socks[slot],buf,len,0);return n<0?0:(uint64_t)n;}
static uint64_t net_recv(uint64_t slot,void *buf,uint64_t len,uint64_t timeout){
    if(slot>=(uint64_t)nsock)return 0;
    struct timeval tv;tv.tv_sec=timeout/1000;tv.tv_usec=(timeout%1000)*1000;
    setsockopt(socks[slot],SOL_SOCKET,SO_RCVTIMEO,&tv,sizeof tv);
    ssize_t n=recv(socks[slot],buf,len,0);return n<=0?0:(uint64_t)n;
}
static uint64_t net_close(uint64_t slot){if(slot<(uint64_t)nsock){close(socks[slot]);socks[slot]=-1;}return 0;}

static uint64_t str_arg(uint64_t p,uint64_t len,char *out,size_t cap){if(len>=cap)len=cap-1;memcpy(out,(void*)p,len);out[len]=0;return len;}

uint64_t kimi_host_syscall(uint64_t n,uint64_t a0,uint64_t a1,uint64_t a2,uint64_t a3,uint64_t a4){
    char path[256],path2[256];
    switch(n){
    case 1: fwrite((void*)a1,1,a2,stdout);fflush(stdout);return a2;
    case 3: return 0;
    case 2: fprintf(stderr,"[host] exit(%llu)\n",(unsigned long long)a0);exit((int)a0);
    case 11: return 1;
    case 20: return (now_ns()-start_ns)/1000000;
    case 21: {struct timespec t={.tv_sec=a0/1000,.tv_nsec=(long)(a0%1000)*1000000};nanosleep(&t,NULL);return 0;}
    case 26: {uint64_t*f=(uint64_t*)a0;time_t t=time(NULL);struct tm tm;gmtime_r(&t,&tm);f[0]=t;f[1]=tm.tm_year+1900;f[2]=tm.tm_mon+1;f[3]=tm.tm_mday;f[4]=tm.tm_hour;f[5]=tm.tm_min;f[6]=tm.tm_sec;return 0;}
    case 4: str_arg(a0,a1,path,sizeof path);return fs_open(path);
    case 5: return fs_read(a0,a1,(void*)a2,a3);
    case 6: return fs_close(a0);
    case 8: return fs_list((void*)a0,a1);
    case 13: str_arg(a0,a1,path,sizeof path);return fs_create(path);
    case 14: return fs_write(a0,a1,(void*)a2,a3);
    case 15: {uint64_t files=0;DIR*d=opendir(diskRoot);if(d){struct dirent*e;while((e=readdir(d)))if(e->d_name[0]!='.')files++;closedir(d);}return (131072ULL<<32)|files;}
    case 18: str_arg(a0,a1,path,sizeof path);return fs_unlink(path);
    case 19: return 131072;
    case 25: str_arg(a0,a1,path,sizeof path);str_arg(a2,a3,path2,sizeof path2);return fs_rename(path,path2);
    case 27: str_arg(a0,a1,path,sizeof path);return fs_stat(path,(uint64_t*)a2);
    case 28: str_arg(a0,a1,path,sizeof path);return fs_listdir(path,(void*)a2,a3);
    case 29: str_arg(a0,a1,path,sizeof path);return fs_mkdir(path);
    case 30: str_arg(a0,a1,path,sizeof path);return fs_rmdir(path);
    case 23: {if(a1<1)return ERROR;uint64_t*o=(uint64_t*)a0;o[0]=1;o[1]=0;o[2]=2;o[3]=0;return 1;}
    case 34:
        switch(a0){
        case 0: return 0;
        case 1: str_arg(a1,a2,path,sizeof path);return net_resolve(path,(void*)a3);
        case 2: return net_connect(a1,a2,a3);
        case 3: return net_send(a1,(void*)a2,a3);
        case 4: return net_recv(a1,(void*)a2,a3,a4);
        case 5: return net_close(a1);
        default: return ERROR;
        }
    case 7: case 9: case 35: case 10: case 24: case 38: return ERROR;
    case 12: return 2048;
    case 22: return 0;
    default: fprintf(stderr,"[host] unhandled syscall %llu\n",(unsigned long long)n);return ERROR;
    }
}

int main(void){
    snprintf(diskRoot,sizeof diskRoot,"/tmp/kimios-fs-XXXXXX");
    if(!mkdtemp(diskRoot)){perror("mkdtemp");return 1;}
    start_ns=now_ns();
    extern void kimi_shell_test(void);
    kimi_shell_test();
    fprintf(stderr,"[host] disk root: %s\n",diskRoot);
    return 0;
}
