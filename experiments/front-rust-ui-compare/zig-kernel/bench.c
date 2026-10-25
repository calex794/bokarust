// Same C harness for both shared libraries. Scope: native HTML microkernel.
#define _POSIX_C_SOURCE 200809L
#include <dlfcn.h>
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <time.h>
#include <sys/resource.h>
typedef size_t (*render_fn)(unsigned char *, size_t, size_t);
static volatile size_t checksum;
static int cmp(const void *a,const void *b) {
    double x=*(const double*)a,y=*(const double*)b;
    return (x>y)-(x<y);
}
static uint64_t now(void) {
    struct timespec t;
    if (clock_gettime(CLOCK_MONOTONIC,&t)) abort();
    return (uint64_t)t.tv_sec*1000000000ULL+(uint64_t)t.tv_nsec;
}
int main(int argc,char **argv) {
    if(argc!=4) return 2;
    size_t count=strtoul(argv[3],NULL,10);
    if(count<3||count>10000) return 2;
    void *lib=dlopen(argv[1],RTLD_NOW|RTLD_LOCAL);
    if(!lib) {fprintf(stderr,"%s\n",dlerror());return 1;}
    render_fn render=(render_fn)dlsym(lib,argv[2]);
    if(!render) {fprintf(stderr,"%s\n",dlerror());return 1;}
    size_t cap=2*1024*1024;
    unsigned char *bytes=malloc(cap);
    if(!bytes) return 1;
    size_t len=render(bytes,cap,count);
    if(!len||len>cap) return 1;
    for(size_t i=0;i<100;i++) checksum^=render(bytes,cap,count);
    size_t reps=count==3?2000:600;
    double *t=calloc(reps,sizeof(double));
    if(!t) return 1;
    for(size_t i=0;i<reps;i++) {
        uint64_t start=now();
        size_t n=render(bytes,cap,count);
        uint64_t end=now();
        if(n!=len) return 1;
        checksum^=n;
        t[i]=(double)(end-start)/1000.0;
    }
    qsort(t,reps,sizeof(double),cmp);
    struct rusage u;
    if(getrusage(RUSAGE_SELF,&u)) return 1;
    printf("{\"count\":%zu,\"samples\":%zu,\"p50_us\":%.5f,\"p95_us\":%.5f,\"bytes\":%zu,\"rss_kib\":%ld}\n",
           count,reps,t[reps/2],t[reps*95/100],len,u.ru_maxrss);
    free(t);free(bytes);dlclose(lib);return 0;
}
