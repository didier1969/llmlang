/* C baseline for aset.lll: in-place array update, the imperative "as fast as C" target.
 * argv lets the harness scale N (and K) for the O(1)-vs-O(n) scaling check.
 * Build [n, n-1, …, 1], apply k left-to-right increment passes IN PLACE (O(1) per element),
 * then checksum. Same observable result as the llmlang kernel. */
#include <stdio.h>
#include <stdlib.h>
int main(int argc, char **argv) {
    long n = argc > 1 ? atol(argv[1]) : 2000;
    long k = argc > 2 ? atol(argv[2]) : 4000;
    long *a = malloc((size_t) n * sizeof(long));
    for (long i = 0; i < n; i++) a[i] = n - i;          /* [n, n-1, …, 1] */
    for (long p = 0; p < k; p++)
        for (long i = 0; i < n; i++) a[i] += 1;         /* k passes, in place */
    long acc = 0;
    for (long i = 0; i < n; i++) acc += a[i];
    printf("%ld\n", acc);
    free(a);
    return 0;
}
