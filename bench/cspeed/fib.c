#include <stdio.h>
long fib(long n){ if(n<2) return n; return fib(n-1)+fib(n-2); }
int main(){ printf("%ld\n", fib(40)); return 0; }
