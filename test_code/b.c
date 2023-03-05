#include "stdio.h"

int main(int argc, char** argv) {
    printf("Starting!\n");

    char b = 'a';
    char a = b / 6;

    for (int i = 0; i < (int)a; ++i) {
        int remainder = i % 2;
        printf("i = %d, remainder: %d\n", i, remainder);
    }

    return 0;
}
