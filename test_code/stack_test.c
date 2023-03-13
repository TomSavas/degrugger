#include <stdio.h>

int sad_branch() {
    printf("Sad branch!\n");
    return 0;
}

int happy_branch() {
    printf("Happy branch!\n");
    return 1;
}

int d(int input) {
    printf("Hello from func d\n");

    int ret = -1;
    if (input % 2 == 0) {
        ret = happy_branch();
    } else {
        ret = sad_branch();
    }
    return ret;
}

int c(int input) {
    printf("Hello from func c\n");
    return d(input);
}

int b(int input) {
    printf("Hello from func b\n");
    return c(input);
}

int a(int input) {
    printf("Hello from func a\n");
    return b(input);
}

int main(int argc, char** argv) {
    for (int i = 0; i < (argc + 2) * 2; ++i) {
        a(i);
    }

    return 0;
}
