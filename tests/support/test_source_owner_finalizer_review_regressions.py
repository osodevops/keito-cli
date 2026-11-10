"""R35 independent finite finalizer cases. No child, signal, thread or env access."""
from pathlib import Path
import sys
sys.path.insert(0, str(Path(__file__).resolve().parent))
import os
import socket
import unittest
from unittest.mock import patch
import source_qa_protocol as p
from source_qa_children import Cancelled

class Parent:
    def __init__(self, send_error=None, close_error=None):
        self.tags=[]; self.closed=False; self.send_error=send_error; self.close_error=close_error
    def send(self, tag):
        self.tags.append(tag)
        if self.send_error is not None: raise self.send_error
    def close(self):
        self.closed=True
        if self.close_error is not None: raise self.close_error

class ReviewCases(unittest.TestCase):
    def test_mixed_clean_failed_collection_is_retained_and_cached(self):
        owner=p.SourceOwner(); calls=[]
        class Manager:
            def __init__(self,name): self.name=name; self.polls=0; self.handle=object(); self.collected=False
            def begin_cleanup(self,end):
                calls.append(('begin',self.name,end))
                if self.name=='a': raise KeyboardInterrupt()
            def poll_cleanup(self,end):
                self.polls+=1; calls.append(('poll',self.name,end))
                if self.name=='a' and self.polls==1: return False
                if self.name=='b' and self.polls==1: return 1
                self.collected=True; return True
        a,b,c=[Manager(x) for x in 'abc']; owner.managers=[a,b,c]
        handles=[x.handle for x in owner.managers]
        with patch.object(p.time,'monotonic_ns',return_value=1), patch.object(p.time,'sleep'):
            owner.finish_managers()
        self.assertEqual([(x[0],x[1]) for x in calls[:3]],[('begin','a'),('begin','b'),('begin','c')])
        self.assertEqual(owner._finished_managers,{id(c)})
        self.assertEqual(owner._failed_managers,{id(a),id(b)})
        self.assertEqual(owner._collected_failed_managers,{id(a),id(b)})
        self.assertTrue(all(x.collected for x in owner.managers))
        self.assertEqual([x.handle for x in owner.managers],handles)
        self.assertEqual(len({x[2] for x in calls}),1)
        before=list(calls); previous=owner.deadline_ns
        with patch.object(p.time,'monotonic_ns',return_value=previous+1): owner.finish_managers()
        self.assertEqual(calls,before); self.assertEqual(owner.deadline_ns,previous)
        self.assertEqual(owner.first_fault,'MANAGER_CLEANUP_BEGIN'); self.assertTrue(owner.unsettled)

    def test_permanent_false_and_throwing_callback_keep_unknown_handles(self):
        owner=p.SourceOwner(); owner.deadline_ns=30; calls=[]
        class Manager:
            def __init__(self,name): self.name=name; self.polls=0; self.handle=object()
            def begin_cleanup(self,end):
                calls.append(('begin',self.name,end))
                if self.name=='error': raise OSError('PRIVATE_BEGIN')
            def poll_cleanup(self,end):
                self.polls+=1; calls.append(('poll',self.name,end))
                if self.name=='error': raise RuntimeError('PRIVATE_POLL')
                return False
        a,b=Manager('error'),Manager('false'); owner.managers=[a,b]
        ticks=iter(range(1,1000))
        with patch.object(p.time,'monotonic_ns',side_effect=lambda:next(ticks)), patch.object(p.time,'sleep'):
            owner.finish_managers()
        self.assertEqual([(x[0],x[1]) for x in calls[:2]],[('begin','error'),('begin','false')])
        self.assertGreater(a.polls,1); self.assertGreater(b.polls,1)
        self.assertEqual(owner.managers,[a,b]); self.assertIsNotNone(a.handle); self.assertIsNotNone(b.handle)
        self.assertFalse(owner._finished_managers); self.assertFalse(owner._collected_failed_managers)
        self.assertEqual(owner.deadline_ns,30); self.assertEqual(owner.first_fault,'MANAGER_CLEANUP_BEGIN')
        self.assertTrue(owner.unsettled)

    def test_late_true_stays_unknown_on_repeated_finish(self):
        owner=p.SourceOwner(); owner.deadline_ns=10; calls=[]
        class Manager:
            def begin_cleanup(self,end): calls.append(('begin',end))
            def poll_cleanup(self,end): calls.append(('poll',end)); return True
        manager=Manager(); owner.managers=[manager]
        with patch.object(p.time,'monotonic_ns',side_effect=[1,2,3,10,11]): owner.finish_managers()
        with patch.object(p.time,'monotonic_ns',return_value=12): owner.finish_managers()
        self.assertEqual([x[0] for x in calls],['begin','poll','begin'])
        self.assertNotIn(id(manager),owner._finished_managers)
        self.assertNotIn(id(manager),owner._collected_failed_managers)
        self.assertEqual(owner.deadline_ns,10); self.assertEqual(owner.first_fault,'MANAGER_CLEANUP_DEADLINE')
        self.assertTrue(owner.unsettled)

    def test_clock_failure_fails_closed_preserves_original_and_current(self):
        for has_original in (False,True):
            with self.subTest(original=has_original):
                owner=p.SourceOwner(); manager=object(); owner.managers=[manager]
                parent=Parent(); owner.parent=parent; previous=object(); owner.previous=previous
                original=OSError('PRIVATE_OPERATION') if has_original else None
                with patch.object(p,'CURRENT',owner), patch.object(p.time,'monotonic_ns',side_effect=OSError('PRIVATE_CLOCK')):
                    if has_original: self.assertFalse(owner.__exit__(OSError,original,None))
                    else:
                        with self.assertRaises(p.ProtocolError) as error: owner.__exit__(None,None,None)
                        self.assertEqual(str(error.exception),'OWNER_FINALIZATION_FAILED')
                    self.assertIs(p.CURRENT,previous)
                self.assertEqual(parent.tags,['FAILED']); self.assertTrue(parent.closed)
                self.assertEqual(owner.managers,[manager]); self.assertFalse(owner._finished_managers)
                self.assertTrue(owner._closed); self.assertTrue(owner.unsettled)
                self.assertEqual(owner.first_fault,'OWNER_OPERATION_FAILED' if has_original else 'OWNER_FINALIZATION_FAILED')

    def test_terminal_and_close_baseexceptions_keep_first_reason(self):
        for original in (None,OSError('PRIVATE_OPERATION'),p.ProtocolError('ORIGINAL')):
            with self.subTest(original=type(original).__name__):
                owner=p.SourceOwner(); owner.parent=Parent(SystemExit('PRIVATE_SEND'),KeyboardInterrupt())
                previous=object(); owner.previous=previous
                with patch.object(p,'CURRENT',owner), patch.object(p.time,'monotonic_ns',return_value=1):
                    if original is None:
                        with self.assertRaises(p.ProtocolError) as error: owner.__exit__(None,None,None)
                        self.assertEqual(str(error.exception),'TERMINAL_WRITE')
                    else: self.assertFalse(owner.__exit__(type(original),original,None))
                    self.assertIs(p.CURRENT,previous)
                self.assertTrue(owner.parent.closed); self.assertTrue(owner._closed)
                self.assertEqual(owner.parent.tags,['SETTLED' if original is None else 'FAILED'])
                self.assertEqual(owner.first_fault,'TERMINAL_WRITE' if original is None else ('ORIGINAL' if isinstance(original,p.ProtocolError) else 'OWNER_OPERATION_FAILED'))

    def test_real_socket_receives_failed_after_operation_or_cancel(self):
        for original in (OSError('PRIVATE_RECEIPT'),Cancelled(15)):
            with self.subTest(original=type(original).__name__):
                left,right=socket.socketpair()
                self.addCleanup(left.close); self.addCleanup(right.close)
                owner=p.SourceOwner(); owner.parent=p.Endpoint(left,'a'*32,budget=owner.budget,on_fault=owner.fail)
                remote=p.Endpoint(right,'a'*32,os.getpid()); owner.parent.send('LIVE')
                remote.receive(); self.assertTrue(remote.live); self.assertFalse(remote.settled)
                with patch.object(p.time,'monotonic_ns',return_value=1):
                    self.assertFalse(owner.__exit__(type(original),original,None))
                with self.assertRaises(p.ProtocolError) as error: remote.receive()
                self.assertEqual(str(error.exception),'PEER_FAILED'); self.assertFalse(remote.settled)
                self.assertEqual(owner.first_fault,'OWNER_OPERATION_FAILED')

    def test_unwaited_forward_ledger_cannot_be_hidden_by_manager_true(self):
        owner=p.SourceOwner()
        child=1 if os.getpid()!=1 else 2
        owner.forward(['REGISTER|{}|{}|{}|1'.format(os.getpid(),child,'b'*32)])
        owner.parent=Parent()
        class Manager:
            def begin_cleanup(self,end): pass
            def poll_cleanup(self,end): return True
        manager=Manager(); owner.managers=[manager]
        with patch.object(p.time,'monotonic_ns',return_value=1):
            with self.assertRaises(p.ProtocolError) as error: owner.__exit__(None,None,None)
        self.assertEqual(str(error.exception),'UNWAITED_FORWARD_REGISTRATION')
        self.assertEqual(owner._finished_managers,{id(manager)})
        self.assertEqual(owner.parent.tags,['FAILED']); self.assertFalse(owner._forward_ledger.complete())
        self.assertTrue(owner.parent.closed); self.assertTrue(owner.unsettled)

    def test_local_four_seconds_and_earlier_inherited_bound_never_extend(self):
        for inherited,expected in ((None,4_000_000_007),(50,50),(9_000_000_000,4_000_000_007)):
            with self.subTest(inherited=inherited):
                owner=p.SourceOwner(); owner.deadline_ns=inherited; calls=[]
                class Manager:
                    def begin_cleanup(self,end): calls.append(end)
                    def poll_cleanup(self,end): calls.append(end); return True
                manager=Manager(); owner.managers=[manager]
                with patch.object(p.time,'monotonic_ns',return_value=7): owner.finish_managers()
                with patch.object(p.time,'monotonic_ns',return_value=8): owner.finish_managers()
                self.assertEqual(owner.deadline_ns,expected); self.assertEqual(calls,[expected,expected])
                self.assertFalse(owner.unsettled); self.assertEqual(owner.managers,[manager])


if __name__ == "__main__":
    unittest.main()
