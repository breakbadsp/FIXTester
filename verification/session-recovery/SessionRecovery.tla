---- MODULE SessionRecovery ----
\* One receiving FIX session and the peer that sends to it. The peer owns its
\* sending numbers and the set of numbers that carried application messages.
\* The receiver reads one TCP stream in order. Any message can be lost before
\* it reaches the stream, and a disconnect loses all bytes in flight. Timers
\* are events with no time.
EXTENDS Naturals, Sequences

CONSTANTS MaxSequence, MaxProbes, MaxWire
VARIABLES
    phase,      \* receiver session state
    expected,   \* receiver next expected MsgSeqNum
    gap,        \* highest MsgSeqNum above an open gap; 0 when no gap is open
    request,    \* BeginSeqNo of the open ResendRequest; 0 when none
    probe,      \* "Pending" while a TestRequest waits for its Heartbeat
    probeId,    \* TestReqID of the last TestRequest; ids are 1..MaxProbes
    answered,   \* ghost: a valid matching Heartbeat reached the receiver
    peerNext,   \* peer next MsgSeqNum
    peerApp,    \* peer MsgSeqNums that carried an application message
    wire,       \* messages in flight to the receiver, oldest first
    delivered,  \* MsgSeqNums that the receiver gave to the application
    waived,     \* MsgSeqNums that a peer gap fill or reset covered
    evidence    \* the last receiver step, for the step invariants

receiver == <<phase, expected, gap, request, probe, probeId, answered, delivered, waived>>
vars == <<receiver, peerNext, peerApp, wire, evidence>>

Numbers == 1..MaxSequence
Range(from, to) == {s \in Numbers : from <= s /\ s < to}
Send(m) == Len(wire) < MaxWire /\ wire' = Append(wire, m)
Msg(type, seq, new, id) == [type |-> type, seq |-> seq, new |-> new, id |-> id]
Note(kind, seq) ==
    [kind |-> kind, seq |-> seq, before |-> expected, phase |-> phase, probe |-> probe]
CloseGap(next) == IF next > gap THEN 0 ELSE gap

Init ==
    /\ phase = "Disconnected"
    /\ expected = 1
    /\ gap = 0
    /\ request = 0
    /\ probe = "Idle"
    /\ probeId = 0
    /\ answered = FALSE
    /\ peerNext = 1
    /\ peerApp = {}
    /\ wire = <<>>
    /\ delivered = {}
    /\ waived = {}
    /\ evidence = [kind |-> "None", seq |-> 0, before |-> 1, phase |-> "Disconnected",
                   probe |-> "Idle"]

\* ---- The peer ----

PeerSends(type, id) ==
    /\ phase \in {"WaitingLogon", "Active"}
    /\ peerNext <= MaxSequence
    /\ Send(Msg(type, peerNext, 0, id))
    /\ peerNext' = peerNext + 1

PeerApp ==
    /\ PeerSends("App", 0)
    /\ peerApp' = peerApp \cup {peerNext}
    /\ UNCHANGED <<receiver, evidence>>

\* A Heartbeat has no TestReqID (0) or echoes one that the peer received.
PeerHeartbeat(id) ==
    /\ id \in 0..probeId
    /\ PeerSends("Heartbeat", id)
    /\ UNCHANGED <<receiver, peerApp, evidence>>

\* An application message sent again with PossDupFlag=Y keeps its MsgSeqNum.
PeerReplay(seq) ==
    /\ phase = "Active"
    /\ seq \in peerApp
    /\ Send(Msg("App", seq, 0, 0))
    /\ UNCHANGED <<receiver, peerNext, peerApp, evidence>>

\* The answer to the open ResendRequest has MsgSeqNum BeginSeqNo. A correct
\* peer sets NewSeqNo to its next number. A lower NewSeqNo is a peer fault.
PeerGapFill(new) ==
    /\ phase = "Active"
    /\ request # 0 /\ request < peerNext
    /\ new \in 1..peerNext
    /\ Send(Msg("GapFill", request, new, 0))
    /\ UNCHANGED <<receiver, peerNext, peerApp, evidence>>

\* SequenceReset without GapFillFlag=Y. The receiver ignores its MsgSeqNum.
PeerReset(new) ==
    /\ phase = "Active"
    /\ new \in 1..peerNext
    /\ Send(Msg("Reset", 0, new, 0))
    /\ UNCHANGED <<receiver, peerNext, peerApp, evidence>>

\* The message never reaches TCP. The others keep their order.
Lose(i) ==
    /\ i \in 1..Len(wire)
    /\ wire' = SubSeq(wire, 1, i - 1) \o SubSeq(wire, i + 1, Len(wire))
    /\ UNCHANGED <<receiver, peerNext, peerApp, evidence>>

\* ---- The receiver ----

\* Ordinary Logon: the checkpoint stays. The peer Logon uses its next number.
Connect ==
    /\ phase = "Disconnected"
    /\ peerNext <= MaxSequence
    /\ phase' = "WaitingLogon"
    /\ wire' = <<Msg("Logon", peerNext, 0, 0)>>
    /\ peerNext' = peerNext + 1
    /\ UNCHANGED <<expected, gap, request, probe, probeId, answered, delivered, waived,
                   peerApp, evidence>>

ReceiveBeforeLogon(m) ==
    /\ phase = "WaitingLogon"
    /\ wire' = Tail(wire)
    /\ IF m.type = "Logon" /\ m.seq >= expected
       THEN /\ phase' = "Active"
            /\ expected' = IF m.seq = expected THEN expected + 1 ELSE expected
            /\ gap' = IF m.seq > expected THEN m.seq ELSE 0
            /\ request' = IF m.seq > expected THEN expected ELSE 0
            /\ evidence' = Note("Logon", m.seq)
       ELSE /\ phase' = "Disconnecting"
            /\ UNCHANGED <<expected, gap, request>>
            /\ evidence' = Note("RefusedLogon", m.seq)
    /\ UNCHANGED <<probe, probeId, answered, delivered, waived, peerNext, peerApp>>

\* A header-valid Heartbeat with the exact TestReqID of the open TestRequest.
Answers(m) == m.type = "Heartbeat" /\ probe = "Pending" /\ m.id = probeId

\* sequence_is_next drops a high message and keeps the highest one. A
\* matching Heartbeat still ends the TestRequest (answers_test_request).
ReceiveAhead(m) ==
    /\ phase = "Active"
    /\ m.type # "Reset" /\ m.seq > expected
    /\ wire' = Tail(wire)
    /\ gap' = IF m.seq > gap THEN m.seq ELSE gap
    /\ request' = IF gap = 0 THEN expected ELSE request
    /\ probe' = IF Answers(m) THEN "Idle" ELSE probe
    /\ answered' = (answered \/ Answers(m))
    /\ evidence' = Note(IF Answers(m) THEN "Reply" ELSE "Ahead", m.seq)
    /\ UNCHANGED <<phase, expected, probeId, delivered, waived, peerNext, peerApp>>

\* A low message here is a possible duplicate: the receiver drops it. A low
\* message without PossDupFlag=Y ends the session (Close).
ReceiveLow(m) ==
    /\ phase = "Active"
    /\ m.type # "Reset" /\ m.seq < expected
    /\ wire' = Tail(wire)
    /\ evidence' = Note("Low", m.seq)
    /\ UNCHANGED <<receiver, peerNext, peerApp>>

\* An in-sequence message. A rejected gap fill still uses its MsgSeqNum: the
\* peer sent a gap fill there instead of the original message.
ReceiveNext(m) ==
    /\ phase = "Active"
    /\ m.type # "Reset" /\ m.seq = expected
    /\ LET filled == m.type = "GapFill" /\ m.new > m.seq
           next == IF filled THEN m.new ELSE expected + 1
       IN /\ expected' = next
          /\ gap' = CloseGap(next)
          /\ request' = IF CloseGap(next) = 0 THEN 0 ELSE request
          /\ delivered' = IF m.type = "App" THEN delivered \cup {m.seq} ELSE delivered
          /\ waived' = CASE filled -> waived \cup Range(m.seq, m.new)
                         [] m.type = "GapFill" -> waived \cup {m.seq}
                         [] OTHER -> waived
          /\ evidence' = Note(CASE m.type = "App" -> "Delivered"
                                [] filled -> "GapFill"
                                [] m.type = "GapFill" -> "RejectedGapFill"
                                [] Answers(m) -> "Reply"
                                [] OTHER -> "Admin", m.seq)
    /\ wire' = Tail(wire)
    /\ probe' = IF Answers(m) THEN "Idle" ELSE probe
    /\ answered' = (answered \/ Answers(m))
    /\ UNCHANGED <<phase, probeId, peerNext, peerApp>>

\* A header-valid Reset without GapFillFlag=Y ignores its own MsgSeqNum.
ReceiveReset(m) ==
    /\ phase = "Active"
    /\ m.type = "Reset"
    /\ wire' = Tail(wire)
    /\ IF m.new >= expected
       THEN /\ expected' = m.new
            /\ gap' = CloseGap(m.new)
            /\ request' = IF CloseGap(m.new) = 0 THEN 0 ELSE request
            /\ waived' = waived \cup Range(expected, m.new)
            /\ evidence' = Note("Reset", 0)
       ELSE /\ UNCHANGED <<expected, gap, request, waived>>
            /\ evidence' = Note("RejectedReset", 0)
    /\ UNCHANGED <<phase, probe, probeId, answered, delivered, peerNext, peerApp>>

\* A gap that stays open gets a new ResendRequest from the expected number.
ResendTimer ==
    /\ phase = "Active" /\ gap # 0
    /\ request' = expected
    /\ UNCHANGED <<phase, expected, gap, probe, probeId, answered, delivered, waived,
                   peerNext, peerApp, wire, evidence>>

SendTestRequest ==
    /\ phase = "Active" /\ probe = "Idle" /\ probeId < MaxProbes
    /\ probe' = "Pending"
    /\ probeId' = probeId + 1
    /\ answered' = FALSE
    /\ UNCHANGED <<phase, expected, gap, request, delivered, waived, peerNext, peerApp, wire,
                   evidence>>

HeartbeatTimeout ==
    /\ phase = "Active" /\ probe = "Pending"
    /\ phase' = "Disconnecting"
    /\ evidence' = Note("HeartbeatTimeout", 0)
    /\ UNCHANGED <<expected, gap, request, probe, probeId, answered, delivered, waived,
                   peerNext, peerApp, wire>>

\* Logon timeout, Logout, or a fatal header or sequence problem.
Close ==
    /\ phase \in {"WaitingLogon", "Active"}
    /\ phase' = "Disconnecting"
    /\ UNCHANGED <<expected, gap, request, probe, probeId, answered, delivered, waived,
                   peerNext, peerApp, wire, evidence>>

\* Bytes in flight are lost. The receive checkpoint stays.
Disconnect ==
    /\ phase # "Disconnected"
    /\ phase' = "Disconnected"
    /\ gap' = 0
    /\ request' = 0
    /\ probe' = "Idle"
    /\ answered' = FALSE
    /\ wire' = <<>>
    /\ evidence' = Note("Disconnect", 0)
    /\ UNCHANGED <<expected, probeId, delivered, waived, peerNext, peerApp>>

Next ==
    \/ PeerApp
    \/ \E id \in 0..MaxProbes : PeerHeartbeat(id)
    \/ \E seq \in Numbers : PeerReplay(seq)
    \/ \E new \in 1..(MaxSequence + 1) : PeerGapFill(new) \/ PeerReset(new)
    \/ \E i \in 1..Len(wire) : Lose(i)
    \/ wire # <<>> /\ LET m == Head(wire) IN
           \/ ReceiveBeforeLogon(m) \/ ReceiveAhead(m) \/ ReceiveLow(m)
           \/ ReceiveNext(m) \/ ReceiveReset(m)
    \/ Connect \/ ResendTimer \/ SendTestRequest \/ HeartbeatTimeout \/ Close \/ Disconnect

\* ---- Properties ----

\* No skip: each peer application message below the expected number reached
\* the application, or the peer itself gap-filled or reset past it.
NoSkip == \A s \in Range(1, expected) : s \in peerApp => s \in delivered \cup waived

\* The receiver never expects a number that the peer has not used.
ExpectedNotAheadOfPeer == expected <= peerNext

DeliveryIsInSequence ==
    evidence.kind = "Delivered" =>
        /\ evidence.phase = "Active"
        /\ evidence.seq = evidence.before
        /\ expected = evidence.before + 1

RejectedResetDoesNotJump ==
    /\ (evidence.kind = "RejectedReset" => expected = evidence.before)
    /\ (evidence.kind = "RejectedGapFill" => expected = evidence.before + 1)

GapIsCurrent ==
    /\ (gap = 0 \/ gap >= expected)
    /\ (phase = "Disconnected" => gap = 0)

\* A Heartbeat that answers the TestRequest prevents the timeout, also when
\* it is above a gap.
NoFalseHeartbeatTimeout == evidence.kind = "HeartbeatTimeout" => ~answered

\* Only the matching Heartbeat or a disconnect ends a TestRequest.
TestRequestEndsOnlyByReply ==
    (evidence.probe = "Pending" /\ probe = "Idle") => evidence.kind \in {"Reply", "Disconnect"}

====
