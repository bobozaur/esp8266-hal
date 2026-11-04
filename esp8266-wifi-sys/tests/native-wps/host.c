/* Optional native WPS regression fixture; see README.md for boundaries. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <ucontext.h>
#include <sys/mman.h>
#include <stdint.h>
#include <string.h>
/* A low-address task stack preserves the SDK's 32-bit control arguments. */
static ucontext_t task, caller;
static void *task_stack;
static int alive;
extern void fixture_tests(void);
void fixture_note(unsigned mode, unsigned count)
{
    if (mode == 3)
        puts("WPS fragment, response, scan and terminal-event cases passed");
    else
        printf("WPS mode %u: %u allocation fault sites with clean retry\n", mode, count);
}

void fixture_fail(unsigned line)
{
    fprintf(stderr, "WPS fixture failed at line %u\n", line);
    exit(1);
}

void fixture_worker_create(void (*entry)(void *))
{
    if (task_stack)
        munmap(task_stack, 65536);
    task_stack = mmap(NULL, 65536, PROT_READ | PROT_WRITE, MAP_PRIVATE | MAP_ANONYMOUS | MAP_32BIT,
        -1, 0);
    if (task_stack == MAP_FAILED)
        abort();
    getcontext(&task);
    task.uc_stack.ss_sp = task_stack;
    task.uc_stack.ss_size = 65536;
    task.uc_link = &caller;
    makecontext(&task, (void (*)(void)) entry, 1, (uintptr_t) 0);
    alive = 1;
}

void fixture_worker_pump(void)
{
    if (!alive)
        abort();
    swapcontext(&caller, &task);
}

void fixture_worker_yield(void)
{
    swapcontext(&task, &caller);
}

void fixture_worker_stop(void)
{
    alive = 0;
    swapcontext(&task, &caller);
    abort();
}

void fixture_worker_delete(void)
{
    alive = 0;
    if (task_stack)
        munmap(task_stack, 65536);
    task_stack = NULL;
}

int main(void)
{
    ucontext_t main_ctx, tests;
    getcontext(&tests);
    tests.uc_stack.ss_sp = mmap(NULL, 131072, PROT_READ | PROT_WRITE,
        MAP_PRIVATE | MAP_ANONYMOUS | MAP_32BIT, -1, 0);
    if (tests.uc_stack.ss_sp == MAP_FAILED)
        abort();
    tests.uc_stack.ss_size = 131072;
    tests.uc_link = &main_ctx;
    makecontext(&tests, fixture_tests, 0);
    swapcontext(&main_ctx, &tests);
    puts("actual canonical WPS C fault transactions passed");
    return 0;
}

void *fixture_guard(const void *data, unsigned size)
{
    static void *mapping;
    if (mapping)
        munmap(mapping, 8192);
    mapping = mmap(NULL, 8192, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mapping == MAP_FAILED || size > 4096 || mprotect(mapping, 4096,
        PROT_READ | PROT_WRITE)) abort();
    void *input = (char *) mapping + 4096 - size;
    memcpy(input, data, size);
    return input;
}
