---- MODULE SendDurability ----
EXTENDS Naturals, FiniteSets

CONSTANTS Profiles, MaxSequence
VARIABLES phase, nextSend, transaction, history, pins, wire

vars == <<phase, nextSend, transaction, history, pins, wire>>
Staging == {"Unpinned", "Pinned"}
Live == {"Ready", "Attempted", "Recorded"}
Rows == Profiles \X (1..MaxSequence)
StagedRow(p) == <<p, nextSend[p]>>
ActiveRow(p) == <<p, nextSend[p] - 1>>

Init ==
    /\ phase = [p \in Profiles |-> "Idle"]
    /\ nextSend = [p \in Profiles |-> 1]
    /\ transaction = 0
    /\ history = {}
    /\ pins = {}
    /\ wire = {}

\* Store::write_transaction acquires the SQLite writer lock.
BeginReservation(p) ==
    /\ phase[p] = "Idle"
    /\ nextSend[p] <= MaxSequence
    /\ transaction = 0
    /\ transaction' = p
    /\ phase' = [phase EXCEPT ![p] = "Unpinned"]
    /\ UNCHANGED <<nextSend, history, pins, wire>>

\* Store::protect publishes pins while the writer lock is held.
PublishPins(p) ==
    /\ phase[p] = "Unpinned"
    /\ phase' = [phase EXCEPT ![p] = "Pinned"]
    /\ pins' = pins \cup {StagedRow(p)}
    /\ UNCHANGED <<nextSend, transaction, history, wire>>

\* SQLite makes the checkpoint and history row visible together.
CommitReservation(p) ==
    /\ phase[p] = "Pinned"
    /\ phase' = [phase EXCEPT ![p] = "Ready"]
    /\ nextSend' = [nextSend EXCEPT ![p] = @ + 1]
    /\ history' = history \cup {StagedRow(p)}
    /\ transaction' = 0
    /\ UNCHANGED <<pins, wire>>

AbortReservation(p) ==
    /\ phase[p] \in Staging
    /\ phase' = [phase EXCEPT ![p] = "Idle"]
    /\ pins' = pins \ {StagedRow(p)}
    /\ transaction' = 0
    /\ UNCHANGED <<nextSend, history, wire>>

\* write_outgoing can reach TCP before finish_write records the result.
AttemptWrite(p) ==
    /\ phase[p] = "Ready"
    /\ phase' = [phase EXCEPT ![p] = "Attempted"]
    /\ wire' \in {wire, wire \cup {ActiveRow(p)}}
    /\ UNCHANGED <<nextSend, transaction, history, pins>>

\* finish_write uses a SQLite write transaction. Failure can stop the batch.
RecordResult(p) ==
    /\ phase[p] = "Attempted"
    /\ transaction = 0
    /\ ActiveRow(p) \in history
    /\ phase' = [phase EXCEPT ![p] = "Recorded"]
    /\ UNCHANGED <<nextSend, transaction, history, pins, wire>>

ReleaseBatch(p) ==
    /\ phase[p] \in {"Attempted", "Recorded"}
    /\ phase' = [phase EXCEPT ![p] = "Idle"]
    /\ pins' = pins \ {ActiveRow(p)}
    /\ UNCHANGED <<nextSend, transaction, history, wire>>

\* Age purge and quota trimming take the same lock before reading pins.
TrimHistory(row) ==
    /\ transaction = 0
    /\ row \in history \ pins
    /\ history' = history \ {row}
    /\ UNCHANGED <<phase, nextSend, transaction, pins, wire>>

Restart ==
    /\ phase' = [p \in Profiles |-> "Idle"]
    /\ transaction' = 0
    /\ pins' = {}
    /\ UNCHANGED <<nextSend, history, wire>>

Next ==
    \/ \E p \in Profiles : BeginReservation(p)
    \/ \E p \in Profiles : PublishPins(p)
    \/ \E p \in Profiles : CommitReservation(p)
    \/ \E p \in Profiles : AbortReservation(p)
    \/ \E p \in Profiles : AttemptWrite(p)
    \/ \E p \in Profiles : RecordResult(p)
    \/ \E p \in Profiles : ReleaseBatch(p)
    \/ \E row \in Rows : TrimHistory(row)
    \/ Restart

WireHasReservedSequence ==
    \A row \in wire : row[2] < nextSend[row[1]]

LiveRowsSurvive ==
    \A p \in Profiles : phase[p] \in Live => ActiveRow(p) \in history

====
