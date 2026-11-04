import contextlib
import io
import unittest
from unittest.mock import Mock
from required_aggregate import require_aggregate

class AggregateTests(unittest.TestCase):
    sha = 'a' * 40
    def run_data(self, **changes):
        run = dict(head_sha=self.sha, event='push', head_branch='main', head_repository={'full_name':'dufangshi/pockymoe-app'}, path='.github/workflows/client-checks.yml', created_at='2026-11-04T00:00:00Z', id=1, run_attempt=2)
        run.update(changes)
        return run
    def check(self, runs, jobs):
        query = Mock(side_effect=[{'workflow_runs':runs}, {'jobs':jobs}])
        with contextlib.redirect_stdout(io.StringIO()):
            require_aggregate('dufangshi/pockymoe-app', self.sha, query=query)
        return query
    def test_aggregate_success_ignores_quarantine_failure(self):
        q = self.check([self.run_data(conclusion='failure')], [{'name':'Client checks complete','status':'completed','conclusion':'success'}, {'name':'Quarantine (non-required)','status':'completed','conclusion':'failure'}])
        self.assertIn('/attempts/2/jobs', q.call_args.args[0])
    def test_missing_failed_and_pending_aggregate_block(self):
        for jobs in [[], [{'name':'Client checks complete','status':'completed','conclusion':'failure'}], [{'name':'Client checks complete','status':'in_progress','conclusion':None}]]:
            with self.assertRaisesRegex(RuntimeError, 'Publication blocked'):
                self.check([self.run_data()], jobs)
    def test_untrusted_or_other_sha_checks_do_not_count(self):
        for run in [self.run_data(event='pull_request'), self.run_data(head_sha='b'*40), self.run_data(head_repository={'full_name':'someone/fork'}), self.run_data(path='.github/workflows/other.yml')]:
            with self.assertRaises(RuntimeError): self.check([run], [])
    def test_newer_failure_replaces_old_success(self):
        latest=self.run_data(id=2,created_at='2026-11-04T01:00:00Z')
        with self.assertRaises(RuntimeError): self.check([self.run_data(),latest], [])
    def test_emergency_override_is_explicit_and_does_not_echo_reason(self):
        output=io.StringIO()
        with contextlib.redirect_stdout(output):
            require_aggregate('dufangshi/pockymoe-app', self.sha, 'private incident detail', query=Mock(side_effect=AssertionError))
        self.assertIn('EMERGENCY CI OVERRIDE', output.getvalue())
        self.assertNotIn('private incident detail', output.getvalue())
        with self.assertRaises(RuntimeError): require_aggregate('dufangshi/pockymoe-app', self.sha, '')

if __name__ == '__main__': unittest.main()
