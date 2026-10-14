//! A pure decoder for the `pgoutput` logical-replication protocol, version 1
//! (the binary messages `pg_logical_slot_peek_binary_changes` returns with
//! `proto_version '1'`), and [`Stream`], which groups the decoded changes by
//! source transaction.
//!
//! Integers are big-endian; strings are NUL-terminated and assumed UTF-8 (a
//! `UTF8` server encoding); tuple values arrive in text form.

use std::collections::HashMap;

/// A malformed or unexpected `pgoutput` message.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum DecodeError {
    #[error("pgoutput: message truncated")]
    Truncated,
    #[error("pgoutput: {0} unexpected trailing bytes")]
    Trailing(usize),
    #[error("pgoutput: unknown message type {0:?}")]
    UnknownMessage(char),
    #[error("pgoutput: unknown tuple column kind {0:?}")]
    UnknownColumn(char),
    #[error("pgoutput: unexpected tuple marker {0:?}")]
    BadMarker(char),
    #[error("pgoutput: string is not NUL-terminated UTF-8")]
    BadString,
    #[error("pgoutput: change for relation {0} before its Relation message")]
    UnknownRelation(u32),
    #[error("pgoutput: {0} outside a transaction")]
    OutsideTxn(&'static str),
}

/// A decode result.
pub type Result<T> = std::result::Result<T, DecodeError>;

/// One column value of a tuple.
#[derive(Debug, Clone, PartialEq)]
pub enum Datum {
    /// SQL NULL (`'n'`).
    Null,
    /// An unchanged TOASTed value (`'u'`): not sent, keep the old one.
    Unchanged,
    /// A value in text output form (`'t'`).
    Text(String),
}

/// A row: one [`Datum`] per column of its [`Relation`].
pub type Tuple = Vec<Datum>;

/// A column of a [`Relation`] message.
#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    /// Part of the replica identity key.
    pub key: bool,
    /// Column name.
    pub name: String,
    /// `pg_type` oid.
    pub type_oid: u32,
    /// `atttypmod`.
    pub type_mod: i32,
}

/// A `Relation` message: the shape of the tuples of relation `id`.
#[derive(Debug, Clone, PartialEq)]
pub struct Relation {
    /// The relation's oid.
    pub id: u32,
    /// Schema name.
    pub namespace: String,
    /// Relation name.
    pub name: String,
    /// `relreplident`: `d`efault, `n`othing, `f`ull or `i`ndex.
    pub replica_identity: u8,
    /// Columns, in tuple order.
    pub columns: Vec<Column>,
}

impl Relation {
    /// The value of column `name` in `tuple` (a tuple of this relation).
    pub fn get<'t>(&self, tuple: &'t Tuple, name: &str) -> Option<&'t Datum> {
        let i = self.columns.iter().position(|c| c.name == name)?;
        tuple.get(i)
    }
}

/// A decoded `pgoutput` message.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq)]
pub enum Message {
    Begin {
        final_lsn: u64,
        commit_ts: i64,
        xid: u32,
    },
    Commit {
        flags: u8,
        commit_lsn: u64,
        /// The end of the commit record: where a consumer resumes.
        end_lsn: u64,
        commit_ts: i64,
    },
    Origin {
        lsn: u64,
        name: String,
    },
    Relation(Relation),
    Type {
        id: u32,
        namespace: String,
        name: String,
    },
    Insert {
        rel: u32,
        new: Tuple,
    },
    Update {
        rel: u32,
        /// The old key (`'K'`) or whole old row (`'O'`), when sent.
        old: Option<Tuple>,
        new: Tuple,
    },
    Delete {
        rel: u32,
        old: Tuple,
    },
    Truncate {
        options: u8,
        rels: Vec<u32>,
    },
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.0.len() < n {
            return Err(DecodeError::Truncated);
        }
        let (head, tail) = self.0.split_at(n);
        self.0 = tail;
        Ok(head)
    }

    fn array<const N: usize>(&mut self) -> Result<[u8; N]> {
        let mut a = [0; N];
        a.copy_from_slice(self.take(N)?);
        Ok(a)
    }

    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16> {
        self.array().map(u16::from_be_bytes)
    }

    fn u32(&mut self) -> Result<u32> {
        self.array().map(u32::from_be_bytes)
    }

    fn i32(&mut self) -> Result<i32> {
        self.array().map(i32::from_be_bytes)
    }

    fn u64(&mut self) -> Result<u64> {
        self.array().map(u64::from_be_bytes)
    }

    fn i64(&mut self) -> Result<i64> {
        self.array().map(i64::from_be_bytes)
    }

    fn string(&mut self) -> Result<String> {
        let n = self
            .0
            .iter()
            .position(|&c| c == 0)
            .ok_or(DecodeError::BadString)?;
        let s = std::str::from_utf8(&self.0[..n]).map_err(|_| DecodeError::BadString)?;
        self.0 = &self.0[n + 1..];
        Ok(s.to_string())
    }

    fn marker(&mut self, ok: &[u8]) -> Result<u8> {
        let m = self.u8()?;
        if ok.contains(&m) {
            Ok(m)
        } else {
            Err(DecodeError::BadMarker(m as char))
        }
    }

    fn tuple(&mut self) -> Result<Tuple> {
        let n = self.u16()?;
        let mut t = Vec::with_capacity(usize::from(n));
        for _ in 0..n {
            t.push(match self.u8()? {
                b'n' => Datum::Null,
                b'u' => Datum::Unchanged,
                b't' => {
                    let len = self.u32()? as usize;
                    let bytes = self.take(len)?;
                    let s = std::str::from_utf8(bytes).map_err(|_| DecodeError::BadString)?;
                    Datum::Text(s.to_string())
                }
                c => return Err(DecodeError::UnknownColumn(c as char)),
            });
        }
        Ok(t)
    }
}

/// Decode one `pgoutput` (protocol 1) message.
pub fn decode(buf: &[u8]) -> Result<Message> {
    let mut r = Reader(buf);
    // Struct fields are evaluated in the order written: the wire order.
    let m = match r.u8()? {
        b'B' => Message::Begin {
            final_lsn: r.u64()?,
            commit_ts: r.i64()?,
            xid: r.u32()?,
        },
        b'C' => Message::Commit {
            flags: r.u8()?,
            commit_lsn: r.u64()?,
            end_lsn: r.u64()?,
            commit_ts: r.i64()?,
        },
        b'O' => Message::Origin {
            lsn: r.u64()?,
            name: r.string()?,
        },
        b'R' => {
            let id = r.u32()?;
            let namespace = r.string()?;
            let name = r.string()?;
            let replica_identity = r.u8()?;
            let n = r.u16()?;
            let mut columns = Vec::with_capacity(usize::from(n));
            for _ in 0..n {
                columns.push(Column {
                    key: r.u8()? & 1 == 1,
                    name: r.string()?,
                    type_oid: r.u32()?,
                    type_mod: r.i32()?,
                });
            }
            Message::Relation(Relation {
                id,
                namespace,
                name,
                replica_identity,
                columns,
            })
        }
        b'Y' => Message::Type {
            id: r.u32()?,
            namespace: r.string()?,
            name: r.string()?,
        },
        b'I' => {
            let rel = r.u32()?;
            r.marker(b"N")?;
            Message::Insert {
                rel,
                new: r.tuple()?,
            }
        }
        b'U' => {
            let rel = r.u32()?;
            let old = match r.marker(b"KON")? {
                b'N' => None,
                _ => {
                    let old = r.tuple()?;
                    r.marker(b"N")?;
                    Some(old)
                }
            };
            Message::Update {
                rel,
                old,
                new: r.tuple()?,
            }
        }
        b'D' => {
            let rel = r.u32()?;
            r.marker(b"KO")?;
            Message::Delete {
                rel,
                old: r.tuple()?,
            }
        }
        b'T' => {
            let n = r.u32()?;
            let options = r.u8()?;
            // Not preallocated: `n` is untrusted.
            let rels = (0..n).map(|_| r.u32()).collect::<Result<_>>()?;
            Message::Truncate { options, rels }
        }
        c => return Err(DecodeError::UnknownMessage(c as char)),
    };
    if r.0.is_empty() {
        Ok(m)
    } else {
        Err(DecodeError::Trailing(r.0.len()))
    }
}

/// A committed source transaction: its row changes (`Insert`, `Update`,
/// `Delete` and `Truncate` messages, in order) and its commit's end LSN.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Txn {
    /// Source transaction id.
    pub xid: u32,
    /// End of its commit record.
    pub end_lsn: u64,
    /// Row changes, in order.
    pub changes: Vec<Message>,
}

/// Groups a sequence of messages (one decoding session) into committed
/// transactions. A transaction still open at the end is not in `done`.
#[derive(Debug, Default)]
pub struct Stream {
    /// Every relation described so far, by oid.
    pub relations: HashMap<u32, Relation>,
    open: Option<Txn>,
    /// Committed transactions, in commit order.
    pub done: Vec<Txn>,
}

impl Stream {
    /// Decode and absorb one message.
    pub fn feed(&mut self, buf: &[u8]) -> Result<()> {
        match decode(buf)? {
            Message::Begin { xid, .. } => {
                if self.open.is_some() {
                    return Err(DecodeError::OutsideTxn("Begin"));
                }
                self.open = Some(Txn {
                    xid,
                    ..Txn::default()
                });
            }
            Message::Commit { end_lsn, .. } => {
                let mut t = self.open.take().ok_or(DecodeError::OutsideTxn("Commit"))?;
                t.end_lsn = end_lsn;
                self.done.push(t);
            }
            Message::Relation(rel) => {
                self.relations.insert(rel.id, rel);
            }
            Message::Origin { .. } | Message::Type { .. } => {}
            change => {
                let rels = match &change {
                    Message::Insert { rel, .. }
                    | Message::Update { rel, .. }
                    | Message::Delete { rel, .. } => std::slice::from_ref(rel),
                    Message::Truncate { rels, .. } => rels.as_slice(),
                    _ => unreachable!("every other message is matched above"),
                };
                if let Some(&id) = rels.iter().find(|id| !self.relations.contains_key(id)) {
                    return Err(DecodeError::UnknownRelation(id));
                }
                let t = self
                    .open
                    .as_mut()
                    .ok_or(DecodeError::OutsideTxn("row change"))?;
                t.changes.push(change);
            }
        }
        Ok(())
    }
}

/// An LSN in Postgres' `X/Y` text form.
pub fn format_lsn(lsn: u64) -> String {
    format!("{:X}/{:X}", lsn >> 32, lsn & 0xFFFF_FFFF)
}

/// Parse an LSN from Postgres' `X/Y` text form.
pub fn parse_lsn(s: &str) -> Option<u64> {
    let (hi, lo) = s.split_once('/')?;
    let hi = u32::from_str_radix(hi, 16).ok()?;
    let lo = u32::from_str_radix(lo, 16).ok()?;
    Some((u64::from(hi) << 32) | u64::from(lo))
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
mod tests {
    use super::*;

    fn s(b: &mut Vec<u8>, x: &str) {
        b.extend_from_slice(x.as_bytes());
        b.push(0);
    }

    fn tuple(b: &mut Vec<u8>, t: &[Datum]) {
        b.extend_from_slice(&(t.len() as u16).to_be_bytes());
        for d in t {
            match d {
                Datum::Null => b.push(b'n'),
                Datum::Unchanged => b.push(b'u'),
                Datum::Text(v) => {
                    b.push(b't');
                    b.extend_from_slice(&(v.len() as u32).to_be_bytes());
                    b.extend_from_slice(v.as_bytes());
                }
            }
        }
    }

    fn txt(v: &str) -> Datum {
        Datum::Text(v.to_string())
    }

    fn begin(xid: u32) -> Vec<u8> {
        let mut b = vec![b'B'];
        b.extend_from_slice(&0x0000_0001_0000_0100u64.to_be_bytes());
        b.extend_from_slice(&(-5i64).to_be_bytes());
        b.extend_from_slice(&xid.to_be_bytes());
        b
    }

    fn commit(end: u64) -> Vec<u8> {
        let mut b = vec![b'C', 0];
        b.extend_from_slice(&(end - 8).to_be_bytes());
        b.extend_from_slice(&end.to_be_bytes());
        b.extend_from_slice(&7i64.to_be_bytes());
        b
    }

    /// `public.sales(quarter text KEY, product text, price float8)`.
    fn relation(id: u32) -> Vec<u8> {
        let mut b = vec![b'R'];
        b.extend_from_slice(&id.to_be_bytes());
        s(&mut b, "public");
        s(&mut b, "sales");
        b.push(b'f');
        b.extend_from_slice(&3u16.to_be_bytes());
        for (key, name, oid) in [
            (1u8, "quarter", 25u32),
            (0, "product", 25),
            (0, "price", 701),
        ] {
            b.push(key);
            s(&mut b, name);
            b.extend_from_slice(&oid.to_be_bytes());
            b.extend_from_slice(&(-1i32).to_be_bytes());
        }
        b
    }

    fn insert(rel: u32, t: &[Datum]) -> Vec<u8> {
        let mut b = vec![b'I'];
        b.extend_from_slice(&rel.to_be_bytes());
        b.push(b'N');
        tuple(&mut b, t);
        b
    }

    fn update(rel: u32, old: Option<(u8, &[Datum])>, new: &[Datum]) -> Vec<u8> {
        let mut b = vec![b'U'];
        b.extend_from_slice(&rel.to_be_bytes());
        if let Some((m, t)) = old {
            b.push(m);
            tuple(&mut b, t);
        }
        b.push(b'N');
        tuple(&mut b, new);
        b
    }

    #[test]
    fn begin_and_commit() {
        assert_eq!(
            decode(&begin(42)),
            Ok(Message::Begin {
                final_lsn: 0x1_0000_0100,
                commit_ts: -5,
                xid: 42
            })
        );
        assert_eq!(
            decode(&commit(0x200)),
            Ok(Message::Commit {
                flags: 0,
                commit_lsn: 0x1F8,
                end_lsn: 0x200,
                commit_ts: 7
            })
        );
    }

    #[test]
    fn relation_columns_and_lookup() {
        let Ok(Message::Relation(r)) = decode(&relation(16384)) else {
            panic!("not a relation")
        };
        assert_eq!((r.id, &*r.namespace, &*r.name), (16384, "public", "sales"));
        assert_eq!(r.replica_identity, b'f');
        let cols: Vec<_> = r
            .columns
            .iter()
            .map(|c| (c.key, &*c.name, c.type_oid))
            .collect();
        assert_eq!(
            cols,
            [
                (true, "quarter", 25),
                (false, "product", 25),
                (false, "price", 701)
            ]
        );
        assert_eq!(r.columns[2].type_mod, -1);
        let t = vec![txt("Q1"), Datum::Null, txt("10")];
        assert_eq!(r.get(&t, "price"), Some(&txt("10")));
        assert_eq!(r.get(&t, "product"), Some(&Datum::Null));
        assert_eq!(r.get(&t, "nope"), None);
    }

    #[test]
    fn insert_text_null_and_unchanged_toast() {
        let t = [txt("Q1"), Datum::Null, Datum::Unchanged, txt("")];
        assert_eq!(
            decode(&insert(7, &t)),
            Ok(Message::Insert {
                rel: 7,
                new: t.to_vec()
            })
        );
        // Non-ASCII text survives.
        let t = [txt("Größe €")];
        assert_eq!(
            decode(&insert(7, &t)),
            Ok(Message::Insert {
                rel: 7,
                new: t.to_vec()
            })
        );
    }

    #[test]
    fn update_without_with_old_row_and_with_key() {
        let new = [txt("Q1"), txt("Widget"), txt("12")];
        assert_eq!(
            decode(&update(7, None, &new)),
            Ok(Message::Update {
                rel: 7,
                old: None,
                new: new.to_vec()
            })
        );
        let old = [txt("Q1"), txt("Widget"), txt("10")];
        for marker in *b"OK" {
            assert_eq!(
                decode(&update(7, Some((marker, &old)), &new)),
                Ok(Message::Update {
                    rel: 7,
                    old: Some(old.to_vec()),
                    new: new.to_vec()
                })
            );
        }
    }

    #[test]
    fn delete_truncate_origin_type() {
        let old = [txt("Q4"), Datum::Null, Datum::Null];
        let mut b = vec![b'D'];
        b.extend_from_slice(&9u32.to_be_bytes());
        b.push(b'K');
        tuple(&mut b, &old);
        assert_eq!(
            decode(&b),
            Ok(Message::Delete {
                rel: 9,
                old: old.to_vec()
            })
        );

        let mut b = vec![b'T'];
        b.extend_from_slice(&2u32.to_be_bytes());
        b.push(1);
        b.extend_from_slice(&5u32.to_be_bytes());
        b.extend_from_slice(&6u32.to_be_bytes());
        assert_eq!(
            decode(&b),
            Ok(Message::Truncate {
                options: 1,
                rels: vec![5, 6]
            })
        );

        let mut b = vec![b'O'];
        b.extend_from_slice(&3u64.to_be_bytes());
        s(&mut b, "node1");
        assert_eq!(
            decode(&b),
            Ok(Message::Origin {
                lsn: 3,
                name: "node1".into()
            })
        );

        let mut b = vec![b'Y'];
        b.extend_from_slice(&600u32.to_be_bytes());
        s(&mut b, "public");
        s(&mut b, "mood");
        assert_eq!(
            decode(&b),
            Ok(Message::Type {
                id: 600,
                namespace: "public".into(),
                name: "mood".into()
            })
        );
    }

    #[test]
    fn malformed_messages_are_errors_not_panics() {
        let msgs = [
            begin(1),
            commit(16),
            relation(3),
            insert(3, &[txt("a"), Datum::Null]),
            update(3, Some((b'O', &[txt("a")])), &[txt("b")]),
        ];
        for m in &msgs {
            for n in 0..m.len() {
                assert!(decode(&m[..n]).is_err(), "prefix {n} of {m:?}");
            }
            let mut long = m.clone();
            long.push(0);
            assert_eq!(decode(&long), Err(DecodeError::Trailing(1)));
        }
        assert_eq!(decode(b"Z"), Err(DecodeError::UnknownMessage('Z')));
        let mut b = insert(3, &[txt("a")]);
        b[5] = b'X';
        assert_eq!(decode(&b), Err(DecodeError::BadMarker('X')));
        let mut b = insert(3, &[txt("a")]);
        b[8] = b'b'; // binary values are protocol 2+ with `binary`
        assert_eq!(decode(&b), Err(DecodeError::UnknownColumn('b')));
        let mut b = insert(3, &[txt("a")]);
        *b.last_mut().unwrap() = 0xFF;
        assert_eq!(decode(&b), Err(DecodeError::BadString));
        let mut b = vec![b'D'];
        b.extend_from_slice(&3u32.to_be_bytes());
        b.push(b'N');
        assert_eq!(decode(&b), Err(DecodeError::BadMarker('N')));
    }

    #[test]
    fn stream_groups_committed_transactions() {
        let row = |q: &str| [txt(q), txt("Widget"), txt("10")];
        let mut st = Stream::default();
        for m in [
            begin(10),
            relation(3),
            insert(3, &row("Q1")),
            update(3, None, &row("Q2")),
            commit(0x100),
            begin(11),
            commit(0x200),
            begin(12),
            insert(3, &row("Q3")),
        ] {
            st.feed(&m).unwrap();
        }
        assert_eq!(st.done.len(), 2, "the open transaction 12 is not done");
        assert_eq!((st.done[0].xid, st.done[0].end_lsn), (10, 0x100));
        assert_eq!(st.done[0].changes.len(), 2);
        assert!(matches!(
            st.done[0].changes[1],
            Message::Update { rel: 3, .. }
        ));
        assert_eq!((st.done[1].xid, st.done[1].end_lsn), (11, 0x200));
        assert!(st.done[1].changes.is_empty());
        assert_eq!(st.relations[&3].name, "sales");
        assert_eq!(st.feed(&begin(13)), Err(DecodeError::OutsideTxn("Begin")));
    }

    #[test]
    fn stream_rejects_out_of_order_messages() {
        let mut st = Stream::default();
        assert_eq!(st.feed(&commit(8)), Err(DecodeError::OutsideTxn("Commit")));
        assert_eq!(
            st.feed(&insert(3, &[])),
            Err(DecodeError::UnknownRelation(3))
        );
        st.feed(&relation(3)).unwrap();
        assert_eq!(
            st.feed(&insert(3, &[])),
            Err(DecodeError::OutsideTxn("row change"))
        );
        let mut b = vec![b'T'];
        b.extend_from_slice(&1u32.to_be_bytes());
        b.push(0);
        b.extend_from_slice(&4u32.to_be_bytes());
        st.feed(&begin(1)).unwrap();
        assert_eq!(st.feed(&b), Err(DecodeError::UnknownRelation(4)));
    }

    #[test]
    fn lsn_text_round_trip() {
        for lsn in [0, 1, 0x1_6B37_4D48, u64::MAX] {
            assert_eq!(parse_lsn(&format_lsn(lsn)), Some(lsn));
        }
        assert_eq!(format_lsn(0x1_6B37_4D48), "1/6B374D48");
        assert_eq!(parse_lsn("0/16B374D8"), Some(0x16B3_74D8));
        for bad in ["", "1", "1/", "/1", "g/1", "1/2/3", "100000000/0"] {
            assert_eq!(parse_lsn(bad), None, "{bad}");
        }
    }
}
