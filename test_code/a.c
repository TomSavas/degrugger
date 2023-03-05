#include "stdio.h"
#include "signal.h"

int main(int argc, char** argv) {
    printf("Starting!\n");
    //raise(SIGTRAP);
    //if (argc < 2) {
    //    return 1;
    //}

    //char a = argv[1][0];
    char b = 'a';
    char a = b / 3;
    if (a % 2) {
        printf("%c (char!) is divisible by 2\n", a);
    } else {
        printf("%c (char!) is not divisible by 2\n", a);
    }

    return 0;
}
