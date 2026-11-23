---- MODULE SessionRecovery ----
EXTENDS Naturals

CONSTANT MaxSequence
VARIABLES phase, expected, gap, evidence

vars == <<phase, expected, gap, evidence>>
Sequences == 1..MaxSequence
Note(kind, seq) == [kind |-> kind, seq |-> seq, before |-> expected, phase |-> phase]

Init ==
    /\ phase = "Disconnected"
    /\ expected = 1
    /\ gap = 0
    /\ evidence = [kind |-> "None", seq |-> 0, before |-> 1, phase |-> phase]

CloseGap(next) == IF next > gap THEN 0 ELSE gap

\* Connect and disconnect preserve the receive checkpoint in ordinary Logon mode.
Connect ==
    /\ phase = "Disconnected"
    /\ phase' = "WaitingLogon"
    /\ UNCHANGED <<expected, gap, evidence>>

Logon(seq) ==
    /\ phase = "WaitingLogon"
    /\ phase' = IF seq < expected THEN "Disconnecting" ELSE "Active"
    /\ expected' = IF seq = expected THEN expected + 1 ELSE expected
    /\ gap' = IF seq > expected THEN seq ELSE 0
    /\ evidence' = Note("Logon", seq)

RejectBeforeLogon ==
    /\ phase = "WaitingLogon"
    /\ phase' = "Disconnecting"
    /\ UNCHANGED <<expected, gap, evidence>>

Timeout ==
    /\ phase \in {"WaitingLogon", "Active"}
    /\ phase' = "Disconnecting"
    /\ UNCHANGED <<expected, gap, evidence>>

Disconnect ==
    /\ phase # "Disconnected"
    /\ phase' = "Disconnected"
    /\ gap' = 0
    /\ UNCHANGED <<expected, evidence>>

\* sequence_is_next drops high messages and remembers the greatest missing sequence.
ReceiveAhead(seq) ==
    /\ phase = "Active"
    /\ seq > expected
    /\ gap' = IF seq > gap THEN seq ELSE gap
    /\ UNCHANGED <<phase, expected, evidence>>

DeliverApplication(seq) ==
    /\ phase = "Active"
    /\ seq = expected
    /\ expected' = seq + 1
    /\ gap' = CloseGap(expected')
    /\ evidence' = Note("Delivered", seq)
    /\ UNCHANGED phase

\* A header-valid Reset without GapFillFlag=Y ignores its own MsgSeqNum.
ResetMode(newSeq) ==
    /\ phase = "Active"
    /\ expected' = IF newSeq >= expected THEN newSeq ELSE expected
    /\ gap' = CloseGap(expected')
    /\ evidence' = Note(IF newSeq >= expected THEN "Reset" ELSE "RejectedReset", 0)
    /\ UNCHANGED phase

\* An in-sequence but rejected gap fill still consumes its own MsgSeqNum.
GapFill(seq, newSeq) ==
    /\ phase = "Active"
    /\ seq = expected
    /\ expected' = IF newSeq > seq THEN newSeq ELSE seq + 1
    /\ gap' = CloseGap(expected')
    /\ evidence' = Note(IF newSeq > seq THEN "GapFill" ELSE "RejectedGapFill", seq)
    /\ UNCHANGED phase

Next ==
    \/ Connect
    \/ \E seq \in Sequences : Logon(seq)
    \/ RejectBeforeLogon
    \/ Timeout
    \/ Disconnect
    \/ \E seq \in Sequences : ReceiveAhead(seq)
    \/ \E seq \in Sequences : DeliverApplication(seq)
    \/ \E newSeq \in 0..MaxSequence : ResetMode(newSeq)
    \/ \E seq \in Sequences, newSeq \in 0..MaxSequence : GapFill(seq, newSeq)

DeliveryIsInSequence ==
    evidence.kind = "Delivered" =>
        /\ evidence.phase = "Active"
        /\ evidence.seq = evidence.before
        /\ expected = evidence.before + 1

RejectedResetDoesNotJump ==
    /\ (evidence.kind = "RejectedReset" => expected = evidence.before)
    /\ (evidence.kind = "RejectedGapFill" => expected = evidence.before + 1)

====
