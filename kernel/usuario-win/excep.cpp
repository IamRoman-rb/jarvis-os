// Excepciones de C++: throw/catch, destructores que corren al desenrollar, catch por tipo y
// re-throw. Compilado con /MT (el runtime de C++ adentro).
#include <cstdio>
#include <stdexcept>
#include <string>

struct Guardia {
    const char *nombre;
    explicit Guardia(const char *n) : nombre(n) { printf("  crea %s\n", nombre); }
    ~Guardia() { printf("  destruye %s\n", nombre); }
};

static int dividir(int a, int b) {
    Guardia g("dividir");
    if (b == 0) throw std::runtime_error("division por cero");
    return a / b;
}

int main() {
    // 1. throw/catch con un destructor en el medio (desenrollado de C++).
    try {
        Guardia g("main");
        dividir(10, 0);
        printf("NO LLEGA\n");
    } catch (const std::runtime_error &e) {
        printf("catch runtime_error: %s\n", e.what());
    }

    // 2. catch por tipo: un int no lo agarra el catch de std::exception.
    try {
        try {
            throw 42;
        } catch (const std::exception &) {
            printf("NO: un int no es std::exception\n");
        }
    } catch (int n) {
        printf("catch int: %d\n", n);
    }

    // 3. re-throw y catch base.
    try {
        try {
            throw std::out_of_range("fuera de rango");
        } catch (const std::exception &) {
            printf("  re-throw\n");
            throw;
        }
    } catch (const std::logic_error &e) {
        printf("catch logic_error: %s\n", e.what());
    }

    // 4. catch (...) atrapa cualquier cosa.
    try {
        throw std::string("texto");
    } catch (...) {
        printf("catch (...) ok\n");
    }
    printf("CPP_OK\n");
    return 0;
}
