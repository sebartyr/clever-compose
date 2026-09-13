db.users.insertMany([{ name: "alice" }, { name: "bob" }]);
db.users.createIndex({ name: 1 }, { unique: true });
