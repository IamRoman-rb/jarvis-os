# Prueba de vino con el Python de Windows del anfitrión (cargo xtask vino --python): la salida
# tiene que ser la misma que con Python en Windows.
import sys, os, json, re, math, datetime, collections
print(sys.version)
print(sys.platform, os.name, os.getcwd())
print(json.dumps({"a": [1, 2]}), re.sub(r"o", "0", "jarvis os"), math.factorial(20))
print(datetime.datetime.now().year, collections.Counter("abracadabra").most_common(2))
with open("desde-python.txt", "w", encoding="utf-8") as f:
    f.write("hola desde Python de Windows\nlínea 2\n")
with open("desde-python.txt", encoding="utf-8") as f:
    print("archivo:", f.read().splitlines())
print("carpeta:", sorted(os.listdir("."))[:5])
os.makedirs("carpeta-py/sub", exist_ok=True)
print("existe:", os.path.isdir("carpeta-py/sub"), os.path.exists("no-existe"))
import hashlib, random, struct, itertools, functools, textwrap, string, decimal, fractions
print(hashlib.sha256(b"jarvis").hexdigest()[:16], struct.pack("<I", 1234).hex())
print(decimal.Decimal("1.1") + decimal.Decimal("2.2"), fractions.Fraction(3, 6))
print(sum(itertools.islice(itertools.count(), 10)), functools.reduce(lambda a, b: a * b, range(1, 6)))
random.seed(1)
print("random:", random.randint(1, 100))
print("FIN OK")
