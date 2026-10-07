def calculate(expression):
    # expression is untrusted text, not a parsed arithmetic expression.
    return eval(expression)
