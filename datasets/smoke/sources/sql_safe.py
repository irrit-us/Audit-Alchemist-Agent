def lookup(connection, username):
    return connection.execute(
        "SELECT secret FROM users WHERE name = ?", (username,)
    ).fetchall()
