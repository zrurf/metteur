import tempfile
import unittest
from pathlib import Path
from workflow_eval_fixtures import tasks, grade

TESTS = '''import unittest
import app
class Tests(unittest.TestCase):
    def test_contract(self):
        if hasattr(app, 'parse_labels'):
            self.assertEqual(app.parse_labels(' a, a,B,b,, '), ['a','B','b'])
            self.assertEqual(app.parse_labels(''), [])
            with self.assertRaises(TypeError): app.parse_labels(None)
        else:
            self.assertTrue(callable(next(value for name,value in vars(app).items() if not name.startswith('_') and callable(value))))
'''

class IndependentGraderTests(unittest.TestCase):
    def test_host_blueprint_exception_requires_exact_bytes(self):
        spec=tasks()[0]
        with tempfile.TemporaryDirectory() as directory:
            root=Path(directory)
            for name,body in spec['files'].items(): (root/name).write_text(body,encoding='utf-8')
            (root/'blueprints').mkdir()
            (root/'blueprints/task.blueprint').write_text('{}')
            host={'blueprints/task.blueprint':'{}'}
            self.assertTrue(grade(spec,root,host)['checks']['scope'])
            (root/'blueprints/task.blueprint').write_text('changed')
            self.assertFalse(grade(spec,root,host)['checks']['scope'])
            (root/'blueprints/task.blueprint').write_text('{}')
            (root/'blueprints/extra.txt').write_text('unrequested')
            self.assertFalse(grade(spec,root,host)['checks']['scope'])

    def test_four_oracles_and_scope_violations(self):
        for spec in tasks():
            with self.subTest(task=spec['id']), tempfile.TemporaryDirectory() as directory:
                root=Path(directory)
                for name,body in spec['files'].items(): (root/name).write_text(body,encoding='utf-8')
                self.assertFalse(grade(spec,root)['passed'], 'unchanged baseline cannot be a delivery')
                (root/'app.py').write_text(spec['oracle'],encoding='utf-8')
                (root/'test_added.py').write_text(TESTS,encoding='utf-8')
                good=grade(spec,root)
                self.assertTrue(good['passed'],good)
                (root/'README.md').write_text('unrequested edit',encoding='utf-8')
                self.assertFalse(grade(spec,root)['passed'])

if __name__ == '__main__': unittest.main()
