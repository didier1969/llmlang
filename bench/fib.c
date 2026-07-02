/* C reference for the "C speed" falsification target (DEC-LLL-022 #2). */
#include <stdio.h>

static long long fib(long long n) {
    if (n == 0) return 0;
    if (n == 1) return 1;
    return fib(n - 1) + fib(n - 2);
}

int main(void) {
    printf("%lld\n", fib(40));
    return 0;
}
