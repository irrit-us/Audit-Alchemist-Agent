import subprocess


def greet(name):
    return subprocess.check_output(["printf", "%s", "Hello " + name], shell=False)
