/* Host-only guard pages and cooperative contexts; no SDK/newlib headers here. */
#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <stdint.h>
#include <string.h>
#include <ucontext.h>
#include <sys/mman.h>

extern void fixture_tests(const char *);
static ucontext_t worker, caller;
static void *worker_stack;
static void (*worker_entry)(void *);
static void *worker_argument;
static int alive, current_worker;

void fixture_note_allocations(unsigned count)
{
    printf("EAP initialization: %u allocation fault sites with clean retry\n", count);
}

void fixture_fail(unsigned line)
{
    fprintf(stderr, "EAP fixture failed at line %u\n", line);
    exit(1);
}

void *fixture_guard(const void *data, unsigned size)
{
    static void *mapping;
    if (mapping)
        munmap(mapping, 8192);
    mapping = mmap(NULL, 8192, PROT_NONE, MAP_PRIVATE | MAP_ANONYMOUS, -1, 0);
    if (mapping == MAP_FAILED || size > 4096 ||
        mprotect(mapping, 4096, PROT_READ | PROT_WRITE))
        abort();
    void *input = (char *)mapping + 4096 - size;
    memcpy(input, data, size);
    return input;
}

static void worker_main(void)
{
    worker_entry(worker_argument);
    abort(); /* An RTOS task must delete itself rather than return. */
}

void fixture_worker_create(void (*entry)(void *), void *argument)
{
    if (alive)
        abort();
    if (worker_stack)
        munmap(worker_stack, 65536);
    worker_stack = mmap(NULL, 65536, PROT_READ | PROT_WRITE,
                        MAP_PRIVATE | MAP_ANONYMOUS | MAP_32BIT, -1, 0);
    if (worker_stack == MAP_FAILED)
        abort();
    worker_entry = entry;
    worker_argument = argument;
    getcontext(&worker);
    worker.uc_stack.ss_sp = worker_stack;
    worker.uc_stack.ss_size = 65536;
    worker.uc_link = &caller;
    makecontext(&worker, worker_main, 0);
    alive = 1;
}

void fixture_worker_pump(void)
{
    if (!alive || current_worker)
        abort();
    current_worker = 1;
    swapcontext(&caller, &worker);
    current_worker = 0;
}

void fixture_worker_yield(void)
{
    current_worker = 0;
    swapcontext(&worker, &caller);
    current_worker = 1;
}

void fixture_worker_stop(void)
{
    alive = current_worker = 0;
    swapcontext(&worker, &caller);
    abort();
}

int fixture_worker_alive(void) { return alive; }
int fixture_worker_is_current(void) { return current_worker; }

static const char *selected_case;
static void run_case(void) { fixture_tests(selected_case); }

int main(int argc, char **argv)
{
    ucontext_t main_context, tests;
    if (argc != 2)
        return 2;
    selected_case = argv[1];
    /* Match native-WPS's low-address control pointers for the 32-bit SDK ABI. */
    getcontext(&tests);
    tests.uc_stack.ss_sp = mmap(NULL, 131072, PROT_READ | PROT_WRITE,
                               MAP_PRIVATE | MAP_ANONYMOUS | MAP_32BIT, -1, 0);
    if (tests.uc_stack.ss_sp == MAP_FAILED)
        abort();
    tests.uc_stack.ss_size = 131072;
    tests.uc_link = &main_context;
    makecontext(&tests, run_case, 0);
    swapcontext(&main_context, &tests);
    printf("actual canonical EAP C: %s passed\n", selected_case);
    return 0;
}
