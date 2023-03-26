#include "stdio.h"
#include "signal.h"

#include "funcs.h"

int main(int argc, char** argv) {
    printf("Starting!\n");

    char b = 'a';
    char a = b / 12;
    float c = (float)b;

    printf("%f to the power of %d is ", c, a);
    c = silly_pow(b, a);
    printf("%f\n", c);

    return 0;
}
