def lookup(connection, username):
    query = "SELECT secret FROM users WHERE name = '" + username + "'"
    return connection.execute(query).fetchall()
