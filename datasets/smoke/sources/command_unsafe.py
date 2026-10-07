import subprocess


def greet(name):
    return subprocess.check_output("printf 'Hello '" + name, shell=True)
