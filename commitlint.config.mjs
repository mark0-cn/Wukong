export default {
  extends: ['@commitlint/config-conventional'],
  defaultIgnores: false,
  rules: {
    'type-enum': [2, 'always', ['feat', 'fix', 'perf', 'refactor', 'docs', 'test', 'build', 'ci', 'chore', 'revert', 'release']],
    'header-max-length': [2, 'always', 100],
    'subject-case': [0],
    'body-max-line-length': [0],
    'footer-max-line-length': [0],
    'subject-specific': [2, 'always'],
    'breaking-change-exclamation-mark': [2, 'always'],
  },
  plugins: [{
    rules: {
      'subject-specific': ({subject}) => {
        const vague = /^(?:fix(?: bugs?| issues?)?|updates?|changes?|improvements?|misc(?:ellaneous)?|wip|test|cleanup|refactor|修复(?:问题|错误)?|更新|优化)[.!\s]*$/i;
        return [Boolean(subject?.trim()) && !vague.test(subject.trim()),
          'Describe the affected behavior or object, e.g. fix(updater): locate Docker credential helpers on Windows'];
      },
    },
  }],
};
