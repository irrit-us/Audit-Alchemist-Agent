import ast


def parse_literal(expression):
    return ast.literal_eval(expression)
