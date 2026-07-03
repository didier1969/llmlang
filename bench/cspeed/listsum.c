#include <stdio.h>
#include <stdlib.h>
typedef struct Node { long v; struct Node* next; } Node;
Node* build(long n){ Node* h=NULL; for(long i=1;i<=n;i++){ Node* x=malloc(sizeof(Node)); x->v=i; x->next=h; h=x; } return h; }
/* recursive, to mirror llmlang's `h + sum(t)` (isolate the Rc-vs-pointer cost) */
long sum(Node* xs){ if(!xs) return 0; return xs->v + sum(xs->next); }
int main(){ Node* xs=build(2000); long acc=0; for(int k=0;k<30000;k++) acc+=sum(xs); printf("%ld\n", acc); return 0; }
