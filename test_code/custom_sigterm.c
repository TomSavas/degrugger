#include <signal.h>
#include <stdio.h>

int a = 0;
void sigterm_handler(int signal) {
    printf("Caught in sigterm_handler! Signal: %d\n", signal); 
    a++;
    if (a == 1) {
        raise(SIGTERM);
    } else {
        a = 0;
    }
}

int main() {
    printf("Starting \n");
    signal(SIGTERM, sigterm_handler);

    printf("Pre SIGTERM\n");
    raise(SIGTERM);
    printf("Post SIGTERM\n");

    return 0;
}
