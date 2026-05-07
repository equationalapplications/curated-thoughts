def standalone(x):
    return x + 1


class Calculator:
    def add(self, a, b):
        return a + b

    def sub(self, a, b):
        return a - b


class Config:
    DEFAULT_TIMEOUT = 30
    MAX_RETRIES = 3
