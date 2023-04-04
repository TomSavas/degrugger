#include <stdio.h>
#include <stdbool.h>

int fib(int n, bool deep) {
    static bool deep_once_hit = false;
    if (deep && !deep_once_hit && n == 0) {
        // Just something to put a bp on with a deep stack
        printf("Deep!\n");
        deep_once_hit = true;
        return 0;
    }

    if (n < 2) 
        return n;

    int a = fib(n - 1, deep);
    int b = fib(n - 2, deep);

    return a + b;
}

int main(int argc, char** argv) {
    int n = 4;
    for (int i = 0; i <= n; ++i) {
        int f = fib(i, i == n);
        printf("%dth of Fib = %d\n", i, f);
    }

    return 0;
}
