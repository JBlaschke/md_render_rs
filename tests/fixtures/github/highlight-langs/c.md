```c
#include <stdio.h>
#include "local.h"
#define MAX(a, b) ((a) > (b) ? (a) : (b))

typedef struct node {
    int value;
    struct node *next;
} node_t;

static const char *names[] = {"a", "b", NULL};

int main(int argc, char **argv) {
    unsigned long n = 10UL;
    float f = 1.5f;
    node_t *head = malloc(sizeof(node_t));
    if (head == NULL) {
        fprintf(stderr, "oom: %s\n", argv[0]);
        return -1;
    }
    for (int i = 0; i < argc; ++i) {
        switch (i) { case 0: break; default: continue; }
    }
    /* comment */
    return MAX(n, 0) & 0x0F;
}
```
