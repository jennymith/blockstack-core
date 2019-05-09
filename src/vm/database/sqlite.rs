use rusqlite::{Connection, OptionalExtension, NO_PARAMS, Row, Savepoint};
use rusqlite::types::ToSql;

use vm::contracts::Contract;
use vm::errors::{Error, ErrType, InterpreterResult as Result, IncomparableError};
use vm::types::{Value, TypeSignature, TupleTypeSignature, AtomTypeIdentifier};

const SQL_FAIL_MESSAGE: &str = "PANIC: SQL Failure in Smart Contract VM.";
const DESERIALIZE_FAIL_MESSAGE: &str = "PANIC: Failed to deserialize bad database data in Smart Contract VM.";

pub struct ContractDatabaseConnection {
    conn: Connection
}

pub struct ContractDatabase <'a> {
    savepoint: Savepoint<'a>
}

pub struct SqliteDataMap {
    map_identifier: i64,
    key_type: TypeSignature,
    value_type: TypeSignature
}

pub trait ContractDatabaseTransacter {
    fn begin_save_point(&mut self) -> ContractDatabase<'_>;
}

impl ContractDatabaseConnection {
    pub fn initialize(filename: &str) -> Result<ContractDatabaseConnection> {
        let mut contract_db = ContractDatabaseConnection::inner_open(filename)?;
        contract_db.execute("CREATE TABLE IF NOT EXISTS maps_table
                      (map_identifier INTEGER PRIMARY KEY AUTOINCREMENT,
                       contract_name TEXT NOT NULL,
                       map_name TEXT UNIQUE NOT NULL,
                       key_type TEXT NOT NULL,
                       value_type TEXT NOT NULL)",
                            NO_PARAMS);
        contract_db.execute("CREATE TABLE IF NOT EXISTS data_table
                      (data_identifier INTEGER PRIMARY KEY AUTOINCREMENT,
                       map_identifier INTEGER NOT NULL,
                       key TEXT NOT NULL,
                       value TEXT)",
                            NO_PARAMS);
        contract_db.execute("CREATE TABLE IF NOT EXISTS contracts
                      (contract_identifier INTEGER PRIMARY KEY AUTOINCREMENT,
                       contract_name TEXT UNIQUE NOT NULL,
                       contract_data TEXT NOT NULL)",
                            NO_PARAMS);

        contract_db.execute("CREATE TABLE IF NOT EXISTS simmed_block_table
                      (simmed_block_height BLOB NOT NULL)",
                            NO_PARAMS);

        let default_height: i128 = 0;
        contract_db.execute("INSERT INTO simmed_block_table (simmed_block_height) VALUES (?)",
                            &[default_height]);

        contract_db.check_schema()?;

        Ok(contract_db)
    }

    pub fn memory() -> Result<ContractDatabaseConnection> {
        ContractDatabaseConnection::initialize(":memory:")
    }

    pub fn open(filename: &str) -> Result<ContractDatabaseConnection> {
        let contract_db = ContractDatabaseConnection::inner_open(filename)?;

        contract_db.check_schema()?;
        Ok(contract_db)
    }

    pub fn check_schema(&self) -> Result<()> {
        let sql = "SELECT sql FROM sqlite_master WHERE name=?";
        let _: String = self.conn.query_row(sql, &["maps_table"],
                                            |row| row.get(0))
            .map_err(|x| Error::new(ErrType::SqliteError(IncomparableError{ err: x })))?;
        let _: String = self.conn.query_row(sql, &["contracts"],
                                            |row| row.get(0))
            .map_err(|x| Error::new(ErrType::SqliteError(IncomparableError{ err: x })))?;
        let _: String = self.conn.query_row(sql, &["data_table"],
                                            |row| row.get(0))
            .map_err(|x| Error::new(ErrType::SqliteError(IncomparableError{ err: x })))?;
        let _: String = self.conn.query_row(sql, &["simmed_block_table"],
                                            |row| row.get(0))
            .map_err(|x| Error::new(ErrType::SqliteError(IncomparableError{ err: x })))?;
        Ok(())
    }

    pub fn inner_open(filename: &str) -> Result<ContractDatabaseConnection> {
        let conn = Connection::open(filename)
            .map_err(|x| Error::new(ErrType::SqliteError(IncomparableError{ err: x })))?;
        Ok(ContractDatabaseConnection {
            conn: conn
        })
    }

    pub fn execute<P>(&mut self, sql: &str, params: P) -> usize
    where
        P: IntoIterator,
        P::Item: ToSql {
        self.conn.execute(sql, params)
            .expect(SQL_FAIL_MESSAGE)
    }

    pub fn begin_save_point_raw(&mut self) -> Savepoint<'_> {
        self.conn.savepoint()
            .expect(SQL_FAIL_MESSAGE)
    }
}

impl ContractDatabaseTransacter for ContractDatabaseConnection {
    fn begin_save_point(&mut self) -> ContractDatabase<'_> {
        let sp = self.conn.savepoint()
            .expect(SQL_FAIL_MESSAGE);
        ContractDatabase::from_savepoint(sp)
    }
}

impl <'a> ContractDatabase <'a> {
    pub fn from_savepoint(sp: Savepoint<'a>) -> ContractDatabase<'a> {
        ContractDatabase {
            savepoint: sp }
    }

    pub fn execute<P>(&mut self, sql: &str, params: P) -> usize
    where
        P: IntoIterator,
        P::Item: ToSql {
        self.savepoint.execute(sql, params)
            .expect(SQL_FAIL_MESSAGE)
    }

    fn query_row<T, P, F>(&self, sql: &str, params: P, f: F) -> Option<T>
    where
        P: IntoIterator,
        P::Item: ToSql,
        F: FnOnce(&Row) -> T {
        self.savepoint.query_row(sql, params, f)
            .optional()
            .expect(SQL_FAIL_MESSAGE)
    }


    fn load_map(&self, contract_name: &str, map_name: &str) -> Result<SqliteDataMap> {
        let (map_identifier, key_type, value_type): (_, String, String) =
            self.query_row(
                "SELECT map_identifier, key_type, value_type FROM maps_table WHERE contract_name = ? AND map_name = ?",
                &[contract_name, map_name],
                |row| {
                    (row.get(0), row.get(1), row.get(2))
                })
            .ok_or(Error::new(ErrType::UndefinedMap(map_name.to_string())))?;

        Ok(SqliteDataMap {
            map_identifier: map_identifier,
            key_type: TypeSignature::deserialize(&key_type),
            value_type: TypeSignature::deserialize(&value_type)
        })
    }

    fn load_contract(&self, contract_name: &str) -> Option<Contract> {
        let contract: Option<String> =
            self.query_row(
                "SELECT contract_data FROM contracts WHERE contract_name = ?",
                &[contract_name],
                |row| {
                    row.get(0)
                });
        match contract {
            None => None,
            Some(ref contract) => Some(
                Contract::deserialize(contract))
        }
    }

    pub fn create_map(&mut self, contract_name: &str, map_name: &str, key_type: TupleTypeSignature, value_type: TupleTypeSignature) {
        let key_type = TypeSignature::new_atom(AtomTypeIdentifier::TupleType(key_type));
        let value_type = TypeSignature::new_atom(AtomTypeIdentifier::TupleType(value_type));

        self.execute("INSERT INTO maps_table (contract_name, map_name, key_type, value_type) VALUES (?, ?, ?, ?)",
                     &[contract_name, map_name, &key_type.serialize(), &value_type.serialize()]);
    }

    pub fn fetch_entry(&self, contract_name: &str, map_name: &str, key: &Value) -> Result<Value> {
        let map_descriptor = self.load_map(contract_name, map_name)?;
        if !map_descriptor.key_type.admits(key) {
            return Err(Error::new(ErrType::TypeError(format!("{:?}", map_descriptor.key_type), (*key).clone())))
        }

        let params: [&ToSql; 2] = [&map_descriptor.map_identifier,
                                   &key.serialize()];

        let sql_result: Option<Option<String>> = 
            self.query_row(
                "SELECT value FROM data_table WHERE map_identifier = ? AND key = ? ORDER BY data_identifier DESC LIMIT 1",
                &params,
                |row| {
                    row.get(0)
                });
        match sql_result {
            None => {
                Ok(Value::Void)
            },
            Some(sql_result) => {
                match sql_result {
                    None => Ok(Value::Void),
                    Some(value_data) => Ok(Value::deserialize(&value_data))
                }
            }
        }
    }

    pub fn set_entry(&mut self, contract_name: &str, map_name: &str, key: Value, value: Value) -> Result<Value> {
        let map_descriptor = self.load_map(contract_name, map_name)?;
        if !map_descriptor.key_type.admits(&key) {
            return Err(Error::new(ErrType::TypeError(format!("{:?}", map_descriptor.key_type), key)))
        }
        if !map_descriptor.value_type.admits(&value) {
            return Err(Error::new(ErrType::TypeError(format!("{:?}", map_descriptor.value_type), value)))
        }

        let params: [&ToSql; 3] = [&map_descriptor.map_identifier,
                                   &key.serialize(),
                                   &Some(value.serialize())];

        self.execute(
            "INSERT INTO data_table (map_identifier, key, value) VALUES (?, ?, ?)",
            &params);

        return Ok(Value::Void)
    }

    pub fn insert_entry(&mut self, contract_name: &str, map_name: &str, key: Value, value: Value) -> Result<Value> {
        let map_descriptor = self.load_map(contract_name, map_name)?;
        if !map_descriptor.key_type.admits(&key) {
            return Err(Error::new(ErrType::TypeError(format!("{:?}", map_descriptor.key_type), key)))
        }
        if !map_descriptor.value_type.admits(&value) {
            return Err(Error::new(ErrType::TypeError(format!("{:?}", map_descriptor.value_type), value)))
        }

        let exists = self.fetch_entry(contract_name, map_name, &key)? != Value::Void;
        if exists {
            return Ok(Value::Bool(false))
        }

        let params: [&ToSql; 3] = [&map_descriptor.map_identifier,
                                   &key.serialize(),
                                   &Some(value.serialize())];

        self.execute(
            "INSERT INTO data_table (map_identifier, key, value) VALUES (?, ?, ?)",
            &params);

        return Ok(Value::Bool(true))
    }

    pub fn delete_entry(&mut self, contract_name: &str, map_name: &str, key: &Value) -> Result<Value> {
        let exists = self.fetch_entry(contract_name, map_name, &key)? != Value::Void;
        if !exists {
            return Ok(Value::Bool(false))
        }

        let map_descriptor = self.load_map(contract_name, map_name)?;
        if !map_descriptor.key_type.admits(key) {
            return Err(Error::new(ErrType::TypeError(format!("{:?}", map_descriptor.key_type), (*key).clone())))
        }

        let none: Option<String> = None;
        let params: [&ToSql; 3] = [&map_descriptor.map_identifier,
                                   &key.serialize(),
                                   &none];

        self.execute(
            "INSERT INTO data_table (map_identifier, key, value) VALUES (?, ?, ?)",
            &params);

        return Ok(Value::Bool(exists))
    }


    pub fn get_contract(&mut self, contract_name: &str) -> Result<Contract> {
        self.load_contract(contract_name)
            .ok_or_else(|| { Error::new(ErrType::UndefinedContract(contract_name.to_string())) })
    }

    pub fn insert_contract(&mut self, contract_name: &str, contract: Contract) {
        if self.load_contract(contract_name).is_some() {
            panic!("Contract already exists {}", contract_name);
        } else {
            self.execute("INSERT INTO contracts (contract_name, contract_data) VALUES (?, ?)",
                         &[contract_name, &contract.serialize()]);
        }
    }

    pub fn get_simmed_block_height(&self) -> Result<i128> {
        let block_height: (i128) =
            self.query_row(
                "SELECT simmed_block_height FROM simmed_block_table LIMIT 1",
                NO_PARAMS,
                |row| row.get(0))
            .expect("Failed to fetch simulated block height");

        Ok(block_height)
    }

    pub fn set_simmed_block_height(&mut self, block_height: i128) {
        self.execute(
            "UPDATE simmed_block_table SET simmed_block_height = ?",
            &[block_height]);
    }

    pub fn roll_back(&mut self) {
        self.savepoint.rollback()
            .expect(SQL_FAIL_MESSAGE);
    }

    pub fn commit(self) {
        self.savepoint.commit()
            .expect(SQL_FAIL_MESSAGE);
    }
}

impl <'a> ContractDatabaseTransacter for ContractDatabase<'a> {
    fn begin_save_point(&mut self) -> ContractDatabase {
        let sp = self.savepoint.savepoint()
            .expect(SQL_FAIL_MESSAGE);
        ContractDatabase::from_savepoint(sp)
    }
}