/* Hilos con la API de Windows directa: CreateThread, _beginthreadex, CRITICAL_SECTION, eventos,
 * semáforos, WaitForMultipleObjects, Interlocked y __declspec(thread). */
#include <process.h>
#include <stdio.h>
#include <windows.h>

static CRITICAL_SECTION cs;
static long suma = 0;
static volatile long atomico = 0;
static HANDLE listo, semaforo;
__declspec(thread) int propio = 0;

static DWORD WINAPI sumador(LPVOID arg) {
    int id = (int)(INT_PTR)arg;
    propio = id * 100;
    for (int i = 0; i < 1000; i++) {
        EnterCriticalSection(&cs);
        suma++;
        LeaveCriticalSection(&cs);
        InterlockedIncrement(&atomico);
    }
    return (DWORD)propio;
}

static unsigned __stdcall espera_evento(void *arg) {
    (void)arg;
    WaitForSingleObject(listo, INFINITE);
    ReleaseSemaphore(semaforo, 1, NULL);
    return 7;
}

int main(void) {
    InitializeCriticalSection(&cs);
    HANDLE h[4];
    DWORD tid;
    for (int i = 0; i < 4; i++) h[i] = CreateThread(NULL, 0, sumador, (LPVOID)(INT_PTR)(i + 1), 0, &tid);
    WaitForMultipleObjects(4, h, TRUE, INFINITE);
    DWORD codes = 0, c;
    for (int i = 0; i < 4; i++) {
        GetExitCodeThread(h[i], &c);
        codes += c;
        CloseHandle(h[i]);
    }
    printf("c suma: %ld, atomico: %ld, codigos: %lu, propio main: %d\n", suma, atomico, codes, propio);

    listo = CreateEventA(NULL, TRUE, FALSE, NULL);
    semaforo = CreateSemaphoreA(NULL, 0, 10, NULL);
    HANDLE e[3];
    for (int i = 0; i < 3; i++) e[i] = (HANDLE)_beginthreadex(NULL, 0, espera_evento, NULL, 0, NULL);
    Sleep(50);
    printf("antes del evento: %s\n", WaitForSingleObject(semaforo, 0) == WAIT_TIMEOUT ? "esperando" : "mal");
    SetEvent(listo);
    for (int i = 0; i < 3; i++) WaitForSingleObject(semaforo, INFINITE);
    WaitForMultipleObjects(3, e, TRUE, INFINITE);
    GetExitCodeThread(e[0], &c);
    printf("evento y semaforo: ok, codigo %lu\n", c);
    DeleteCriticalSection(&cs);
    printf("HILOS_C_OK\n");
    return 0;
}
